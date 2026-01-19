//! The `RateExecutor` and its components, providing a rate-controlled,
//! stage-based execution model.
//!
//! The `RateExecutor` implements a token-bucket governor driven by a list of
//! [`Stage`]s. Each `Stage` defines a target requests-per-second (RPS) and a
//! duration over which the governor will smoothly interpolate from the previous
//! rate to the stage's target.
//!
//! This design separates **rate generation** (governor task) from **work
//! execution** (worker tasks) and keeps the hot-path in workers focused on
//! calling the user's `action`.
//!
//! # High-level flow
//! 1. A shared execution context is created, holding shared state for startup
//!    and shutdown coordination.
//! 2. A "token pool" (implemented via `tokio::sync::Semaphore`) is created to
//!    manage the number of available requests.
//! 3. The governor task is spawned. It adds tokens to this pool periodically
//!    based on the defined `Stage`s.
//! 4. N worker tasks are spawned. Each worker repeatedly:
//!    - waits for the start signal,
//!    - tries to acquire a token from the pool,
//!    - when a token is acquired, calls the `action()` future to produce a
//!      `Metric`, and consumes it into a worker-local `Aggregate`.
//! 5. When the governor finishes all stages, it signals shutdown. The executor
//!    collects aggregates from all workers, merging them to produce the final
//!    result.
//!
//! # Tuning knobs
//! - `tick` (Duration): Granularity of governor updates. Smaller ticks reduce
//!   quantization error but cause more wakeups and overhead. Typical: 10–200ms.
//! - `bucket_capacity` (u64): Maximum stored tokens for absorbing bursts.
//! - `workers` (usize): Number of worker tasks. Default is `num_cpus * 120`.
//!
//! # Mathematical behavior of the governor
//! For a given stage with `start_rate` (previous rate) and `end_rate`
//! (stage.target) over `duration`, at time `elapsed` the instantaneous rate `r(t)`
//! is computed by linear interpolation:
//!
//! ```text
//! t = elapsed / duration
//! r(t) = start_rate + (end_rate - start_rate) * t
//! ```
//!
//! The governor then computes how many tokens to add in a `tick`:
//!
//! ```text
//! add_f = r(t) * tick_seconds
//! add_total = floor(add_f + fractional)
//! fractional = (add_f + fractional) - add_total
//! ```
//!
//! `add_total` tokens are added to the pool (saturating at `bucket_capacity`).
//! This spreads the continuous rate into discrete request tokens while preserving
//! the long-term average.use tokio::sync::watch::{self, Receiver};
use tokio::time::Instant;
use tokio::{sync::Notify, task::JoinHandle};
use typed_builder::TypedBuilder;

use super::Executor;
use crate::{aggregate::Aggregate, scenario::Scenario};
use internals::*;

use futures::future::join_all;
use std::{future::Future, sync::Arc, time::Duration};

/// A stage defines a target RPS and how long to ramp to that target.
///
/// Use `Stage::new(Duration::from_secs(10), 100.0)` to ramp to 100 RPS over 10s.
/// If `duration` is `Duration::ZERO`, the executor will jump to the `target`
/// RPS instantly.
///
/// Note: a stage with Duration::ZERO **only** updates the governor's instantaneous rate for subsequent stages; it does not itself add tokens. If you want to sustain a rate, use a stage with non-zero duration.
#[derive(Clone, Copy, Debug)]
pub struct Stage {
    pub duration: Duration,
    /// Requests per second
    pub target: f64,
}

impl Stage {
    pub fn new(duration: Duration, target: f64) -> Self {
        Self { duration, target }
    }
}

/// The semaphore implementation uses 3 bits of usize for flags.
/// Any value greater than this will be capped to avoid crashing
/// the whole thing.
const MAX_TOKENS: usize = usize::MAX >> 3;

/// Executor that drives a token-bucket governed by ramp stages and spawns worker tasks.
///
/// This executor implements a token-bucket rate-limiting strategy using a
/// [`tokio::sync::Semaphore`].
///
/// - A central "governor" task (`token_governor_task`) runs in the background, adding
///   permits (tokens) to the semaphore at a rate determined by the defined `stages`.
/// - The governor ticks every `tick` duration, calculating the correct number of tokens
///   to add based on a linear interpolation between the previous stage's rate and the
///   current one.
/// - A pool of `workers` tasks is spawned. Each worker asynchronously waits to
///   acquire a permit from the semaphore.
/// - Once a permit is acquired, the worker "forgets" it (preventing it from being
///   returned) and executes the `action`.
/// - `bucket_capacity` bounds the maximum number of permits that can be "saved up" in
///   the semaphore, allowing for controlled bursts.
///
/// # Tuning Knobs
///
/// - `tick`: Granularity of governor updates. Smaller ticks (e.g., 10ms) provide
///   smoother rate control but increase scheduler overhead. Larger ticks
///   (e.g., 200ms) are more coarse.
/// - `bucket_capacity`: Maximum stored tokens for absorbing bursts. `usize::MAX`
///   (default) effectively means an unlimited bucket. A smaller capacity
///   enforces a stricter rate limit.
/// - `workers`: Number of worker tasks. Each worker is an async task. The default
///   (`num_cpus * 120`) is tuned for high-throughput async I/O workloads.
#[derive(TypedBuilder)]
pub struct RateExecutor {
    /// The sequence of rate-control stages to execute.
    pub stages: Vec<Stage>,
    /// The granularity of the governor's rate-control tick.
    #[builder(default = Duration::from_millis(100))]
    pub tick: Duration,
    /// The maximum number of tokens allowed in the bucket.
    #[builder(default = MAX_TOKENS)]
    pub bucket_capacity: usize,
    /// The number of concurrent worker tasks to spawn.
    pub workers: usize,
}

impl<A, F, Fut> Executor<A, F, Fut> for RateExecutor
where
    Self: Send + Sync + Sized,
    A: Aggregate + 'static,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    type Error = Box<dyn std::error::Error>;
    async fn exec(&self, scenario: &Scenario<A, F, Fut>) -> Result<A, Self::Error> {
        let (ctx, shutdown_tx) = ExecutionContext::new();
        tracing::info!("Spawning token governor task...");
        let governor = tokio::spawn(token_governor_task(
            ctx.clone(),
            self.stages.clone(),
            self.tick,
            self.bucket_capacity,
        ));

        tracing::info!("Spawning {} workers...", self.workers);
        let handles = spawn_workers(ctx.clone(), self.workers, scenario.action.clone()).await;

        tracing::info!("Running scenario: {}!", scenario.name);
        ctx.start.notify_waiters();

        // The governor task ending means it's all over
        governor.await.expect("Error in token governor task");
        tracing::info!("Governor finished, signaling shutdown...");
        shutdown_tx.send(true)?;

        tracing::info!("Retrieving data from workers...");
        let aggs: Vec<A> = join_all(handles)
            .await
            .into_iter()
            .map(|res| match res {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("Worker panicked with error: {e}");
                    // instead of crashing, lets use return a zeroed agg
                    // this way we dont lose all the data due to one worker panic
                    A::new()
                }
            })
            .collect();

        tracing::info!("Processing results...");
        let mut final_agg = A::new();
        for agg in aggs {
            final_agg.merge(agg);
        }

        tracing::info!("Done running scenario: {}!", scenario.name);
        Ok(final_agg)
    }
}

#[cfg(feature = "internals")]
pub use internals::*;

/// Internal components for the `RateExecutor`.
/// Encapsulated in a module to allow conditional exposure via `#[cfg(feature = "internals")]`.
mod internals {
    use super::*;
    use tokio::sync::{
        watch::{channel, Receiver, Sender},
        Semaphore,
    };

    /// Shared execution state for the governor and all worker tasks.
    #[derive(Clone)]
    pub struct ExecutionContext {
        /// Broadcasts the signal to start the test.
        pub start: Arc<Notify>,
        /// Broadcasts the signal to stop all tasks.
        pub shutdown: Receiver<bool>,
        /// The token bucket, implemented as a semaphore.
        /// Workers acquire permits, and the governor adds them.
        pub tokens: Arc<Semaphore>,
    }

    impl ExecutionContext {
        pub fn new() -> (Self, Sender<bool>) {
            let (tx, rx) = channel(false);
            (
                Self {
                    start: Arc::new(Notify::new()),
                    shutdown: rx,
                    tokens: Arc::new(Semaphore::new(0)),
                },
                tx,
            )
        }
    }

    /// Governor task that adds tokens to the shared semaphore
    /// according to the defined stages.
    pub async fn token_governor_task(
        mut ctx: ExecutionContext,
        stages: Vec<Stage>,
        tick: Duration,
        bucket_capacity: usize,
    ) {
        let main_task = || async {
            let mut rate = 0.0;
            let mut fractional = 0.0;

            ctx.start.notified().await;
            tracing::debug!("Governor task started.");
            let j = stages.len();
            for (i, stage) in stages.into_iter().enumerate() {
                tracing::info!("Starting stage: {i}/{j}");
                // Instantly jump to target rate.
                // This allows handling spikes or starting at a non-zero rate.
                if stage.duration.is_zero() {
                    rate = stage.target;
                    continue;
                }

                let stage_start = Instant::now();
                let mut next_tick = Instant::now();
                let start_rate = rate;
                let end_rate = stage.target;

                loop {
                    let elapsed = Instant::now().duration_since(stage_start);
                    if elapsed >= stage.duration {
                        break;
                    }
                    next_tick += tick;

                    let (add_total, f) = calc_token_limit(
                        elapsed,
                        stage.duration,
                        start_rate,
                        end_rate,
                        fractional,
                        tick,
                    );
                    fractional = f;

                    if add_total > 0 {
                        let avail = ctx.tokens.available_permits();
                        if avail < bucket_capacity {
                            // Only add tokens up to the bucket capacity
                            let free_cap = bucket_capacity - avail;
                            let add = add_total.min(free_cap);
                            if add > 0 {
                                ctx.tokens.add_permits(add);
                            }
                        }
                    }
                    tokio::time::sleep_until(next_tick).await;
                }
                // Ensure the rate for the *next* stage starts from the
                // exact target of *this* stage, preventing rounding errors.
                rate = end_rate;

                tracing::info!("Finishing stage: {i}/{j}");
            }
        };

        tokio::select! {
            _ = main_task() => {
                tracing::debug!("Governor task finished all stages.");
            }
            _ = ctx.shutdown.wait_for(|b|*b) => {
                tracing::debug!("Governor received shutdown signal.");
            }
        };
    }

    /// Pure function to calculate the number of tokens to add this tick.
    ///
    /// It performs linear interpolation of the rate and carries any
    /// fractional tokens over to the next tick to maintain the long-term
    /// average rate.
    ///
    /// Returns `(tokens_to_add, next_fractional_part)`.
    pub fn calc_token_limit(
        elapsed: Duration,
        stage_duration: Duration,
        start_rate: f64,
        end_rate: f64,
        fractional: f64,
        tick: Duration,
    ) -> (usize, f64) {
        // Interpolation factor [0.0..1.0]
        let t = (elapsed.as_secs_f64() / stage_duration.as_secs_f64()).min(1.0);
        // Linear interpolation of the rate
        let tick_rate = start_rate + (end_rate - start_rate) * t;
        // Tokens to add this tick (as a float)
        let add_f = tick_rate * tick.as_secs_f64();

        let add_total_f = (add_f + fractional).floor();
        let fractional = (add_f + fractional) - (add_total_f);

        //  Safely convert f64 to usize, saturating at the semaphore's hard limit
        // to prevent panics.
        let add_total = if add_total_f >= (MAX_TOKENS as f64) {
            MAX_TOKENS
        } else if add_total_f < 0.0 {
            0
        } else {
            add_total_f as usize
        };

        (add_total, fractional)
    }

    /// Spawns `workers` Tokio tasks, each acting as a test worker.
    ///
    /// Each worker waits for the start signal, then enters a loop to
    /// acquire tokens and execute the `action`.
    pub async fn spawn_workers<A, F, Fut>(
        ctx: ExecutionContext,
        workers: usize,
        action: F,
    ) -> Vec<JoinHandle<A>>
    where
        A: Aggregate + 'static,
        F: Fn() -> Fut + Send + Sync + Clone + 'static,
        Fut: Future<Output = A::Metric> + Send,
    {
        (0..workers)
            .map(|i| {
                let mut ctx = ctx.clone();
                let action = action.clone();
                tokio::spawn(async move {
                    let mut agg = A::new();
                    tracing::debug!("Worker {i} spawned.");

                    let main_task = async {
                        ctx.start.notified().await;
                        tracing::debug!("Worker {i} started.");

                        loop {
                            let permit = match ctx.tokens.clone().acquire_owned().await {
                                Ok(p) => p,
                                Err(_) => {
                                    tracing::debug!(
                                        "Worker {i} failed to acquire token (semaphore closed).",
                                    );
                                    break;
                                }
                            };

                            // We "forget" the permit so it's not returned to the
                            // semaphore. The governor is solely responsible for
                            // adding permits.
                            permit.forget();

                            let metric = action().await;
                            agg.consume(&metric);
                        }
                    };

                    tokio::select! {
                        _ = main_task => {},
                        _ = ctx.shutdown.wait_for(|b| *b) => {
                        }
                    };

                    tracing::debug!("Worker {i} shutting down.",);
                    agg
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Metric;

    // A simple metric for testing.
    #[derive(Clone, PartialEq, PartialOrd)]
    struct EmptyMetric;

    impl Metric for EmptyMetric {}

    // A simple aggregate for testing.
    #[derive(Clone)]
    struct EmptyAggregate;

    impl Aggregate for EmptyAggregate {
        type Metric = EmptyMetric;
        fn new() -> Self {
            Self {}
        }
        fn consume(&mut self, _: &Self::Metric) {}
        fn merge(&mut self, _: Self) {}
    }

    #[tokio::test]
    async fn spawn_expected_number_of_workers() {
        let n = 10;
        let (ctx, _) = ExecutionContext::new();
        let action = || async { EmptyMetric {} };
        let workers: Vec<JoinHandle<EmptyAggregate>> = spawn_workers(ctx, n, action).await;

        assert_eq!(workers.len(), n);
    }

    mod calc_token_limit {
        use super::*;

        #[test]
        fn linearity() {
            let mut end_rate = 100.;
            let mut expected_t = 1;
            for _ in 0..10 {
                let (t, f) = calc_token_limit(
                    Duration::from_secs(1),
                    Duration::from_secs(10),
                    0.,
                    end_rate,
                    0.,
                    Duration::from_millis(100),
                );

                assert_eq!(t, expected_t);
                // as they are always powers of 10 there should never be a fractional carry
                assert_eq!(f, 0.);

                end_rate *= 10.;
                expected_t *= 10;
            }
        }

        #[test]
        fn fractional_accumulation() {
            let dur = 10;
            let start_rate = 12.5;
            let end_rate = 12.5;
            let mut facc = 0.;

            let expected_fs = [0.25, 0.5, 0.75, 0.];

            for i in 0..10 {
                let (t, f) = calc_token_limit(
                    Duration::from_secs(1),
                    Duration::from_secs(dur),
                    start_rate,
                    end_rate,
                    facc,
                    Duration::from_millis(100),
                );
                facc = f;

                let expected_f = expected_fs[i % 4];
                let expected_t = if expected_f == 0. { 2 } else { 1 };
                assert_eq!(t, expected_t);
                assert_eq!(f, expected_f)
            }
        }

        #[test]
        fn ramp_down() {
            let stage_duration = Duration::from_secs(10);
            let tick = Duration::from_millis(100);
            let start_rate = 100.0;
            let end_rate = 0.0;

            for i in 0..10 {
                let elapsed = Duration::from_secs(i);
                let (t, f) =
                    calc_token_limit(elapsed, stage_duration, start_rate, end_rate, 0.0, tick);
                let expected_t = (10 - i) as usize;
                assert_eq!(t, expected_t);
                assert_eq!(f, 0.0);
            }

            let (t_end, f_end) = calc_token_limit(
                stage_duration,
                stage_duration,
                start_rate,
                end_rate,
                0.0,
                tick,
            );
            assert_eq!(t_end, 0);
            assert_eq!(f_end, 0.0);
        }

        #[test]
        fn hold_steady() {
            let stage_duration = Duration::from_secs(10);
            let tick = Duration::from_millis(100);
            let start_rate = 100.;
            let end_rate = start_rate;

            for i in 0..10 {
                let elapsed = Duration::from_secs(i);
                let (t, f) =
                    calc_token_limit(elapsed, stage_duration, start_rate, end_rate, 0.0, tick);
                let expected_t = 10;
                assert_eq!(t, expected_t);
                assert_eq!(f, 0.0);
            }
        }

        #[test]
        fn ramp_up() {
            let stage_duration = Duration::from_secs(10);
            let tick = Duration::from_millis(100);
            let start_rate = 0.;
            let end_rate = 100.;

            for i in 0..10 {
                let elapsed = Duration::from_secs(i);
                let (t, f) =
                    calc_token_limit(elapsed, stage_duration, start_rate, end_rate, 0., tick);
                let expected_t = (i) as usize;
                assert_eq!(t, expected_t);
                assert_eq!(f, 0.);
            }
        }

        #[test]
        fn elapsed_over_duartion_cap_at_end_rate() {
            for i in 0..10 {
                let elapsed = 10 + i;
                let (t, f) = calc_token_limit(
                    Duration::from_secs(elapsed),
                    Duration::from_secs(10),
                    0.,
                    100.,
                    0.,
                    Duration::from_millis(100),
                );

                // the rate should never change get over a max (10 in this case) if elapsed over duration
                assert_eq!(t, 10);
                assert_eq!(f, 0.);
            }
        }

        #[test]
        fn negative_value_returns_0() {
            let (t, f) = calc_token_limit(
                Duration::from_secs(1),
                Duration::from_secs(10),
                -100.,
                -100.,
                0.,
                Duration::from_millis(100),
            );

            // The function should cap negative token counts at 0
            assert_eq!(t, 0);
            assert_eq!(f, 0.0);
        }

        #[test]
        fn extreme_rate_cap_at_max_tokens() {
            let (t, f) = calc_token_limit(
                Duration::from_secs(1),
                Duration::from_secs(1),
                f64::MAX,
                f64::MAX,
                0.,
                Duration::from_secs(1),
            );

            assert_eq!(t, MAX_TOKENS);
            assert_eq!(f, 0.);
        }
    }
}
