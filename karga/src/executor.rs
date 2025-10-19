//! Executor — orchestration of runtime execution and rate control
//!
//! The `Executor` trait is the runtime hook that executes a `Scenario`. Different
//! executors provide different execution strategies: sequential, concurrent,
//! distributed, or token-bucket-based.
//!
//! The `StageExecutor` below implements a token-bucket governor driven by a list of
//! `Stage`s. Each `Stage` defines a target requests-per-second (RPS) and a duration
//! over which the governor will smoothly interpolate from the previous rate to the
//! stage's target. The governor periodically adds tokens to a shared atomic counter
//! and worker tasks claim tokens (one token per request) using CAS.
//!
//! This design separates **rate generation** (governor) from **work execution**
//! (workers) and keeps the hot-path in workers focused on calling the user's `action`.
//!
//! # High-level flow
//! 1. Spawn a governor task that increments the shared `tokens` counter according to
//!    the current, interpolated rate.
//! 2. Spawn N worker tasks. Each worker repeatedly:
//!    - waits until the benchmark `start` flag is set,
//!    - tries to claim a token (CAS on `tokens`), sleeping briefly if none are available,
//!    - when a token is claimed, calls the `action()` future to produce a `Metric`, and
//!      consumes it into a worker-local `Aggregate`.
//! 3. When the governor finishes all stages it exits; the executor sets `shutdown` and
//!    collects aggregates from all workers, merging them to produce the final result.
//!
//! # Tuning knobs
//! - `tick` (Duration): granularity of governor updates. Smaller ticks reduce
//!   quantization error but cause more wakeups and overhead. Typical values: 10–200ms.
//! - `bucket_capacity` (u64): maximum stored tokens for absorbing bursts. A small
//!   capacity limits bursts; a larger capacity allows short spikes.
//! - `workers` (usize): number of worker tasks. Each worker is an async task that
//!   awaits tokens and runs the provided `action`. Default is `num_cpus * 120`.
//!
//! # Notes about correctness & robustness
//! - `merge` on aggregates must be associative & commutative; worker-local aggregates
//!   are merged in arbitrary order.
//! - Panics inside a worker task will cause that task to abort; the current executor
//!   uses `expect` on worker joins which will propagate panics as program panics.
//! - The governor uses floating-point interpolation to compute per-tick additions.
//!   To avoid starving fractional contributions, a small `fractional` accumulator is
//!   carried across ticks and converted to integer tokens when it accumulates enough
//!   fractional parts.
//!
//! # Mathematical behavior of the governor
//! For a given stage with `start_rate` (previous rate) and `end_rate` (stage.target)
//! over `duration`, at time `elapsed` the instantaneous rate `r(t)` is computed by
//! linear interpolation:
//!
//! ```text
//! t = elapsed / duration
//! r(t) = start_rate + (end_rate - start_rate) * t
//! ```
//!
//! The governor then computes how many tokens to add in a tick of length `tick`:
//!
//! ```text
//! add_f = r(t) * tick_seconds
//! add_total = floor(add_f + fractional)
//! fractional = (add_f + fractional) - add_total
//! ```
//!
//! `add_total` tokens are added atomically (saturating at `bucket_capacity`). This
//! spreads the continuous rate into discrete request tokens while preserving the
//! long-term average.
//!
//! # Deep dive & implementation notes
//!
//! ## Fractional carrying
//! The `fractional` accumulator ensures that small fractional contributions (e.g. 0.3
//! tokens per tick) are not lost. By accumulating these fractions across ticks, the
//! governor preserves the expected total number of tokens over time and avoids bias.
//!
//! ## Why CAS loops?
//! The token counter is an `AtomicU64`. To avoid races when multiple governor ticks
//! or workers update it concurrently we use a CAS (`compare_exchange`) loop. This
//! ensures correctness without a global lock and provides good performance under
//! contention.
//!
//! ## Worker token-claim strategy
//! Workers perform a fast `load` and, if tokens are available, attempt a `compare_exchange`
//! to claim one token. If the `compare_exchange` fails (another worker claimed the token),
//! the worker retries. If no tokens are available, workers sleep briefly (1ms) to avoid
//! busy-waiting. This sleep value is a trade-off: smaller sleeps reduce latency but
//! increase CPU usage; larger sleeps reduce CPU but increase token claim latency.
//!
//! ## Start & shutdown coordination
//! `start` and `shutdown` are `AtomicBool`s used to coordinate lifecycle. Workers spin
//! until `start` is true, then begin normal operation. The governor finishing its
//! stages signals that the run is complete; the executor sets `shutdown` and waits for
//! workers to drain.
//!
//! # Common pitfalls & recommendations
//! - **Do not perform blocking I/O inside the `action`.** Use async clients and non-blocking
//!   primitives. Blocking inside the action will stall workers and distort the RPS.
//! - **Avoid heavy allocations per action invocation.** Allocate buffers outside the action
//!   when possible and reuse them.
//! - **Tune `tick` for your workload.** Very small ticks (e.g., <10ms) increase scheduler
//!   churn. Very large ticks (e.g., >1s) cause coarse-grained rate control and visible
//!   sawtooth effects in throughput.
//! - **Choose workers count carefully.** Too few workers limit concurrency; too many waste
//!   memory and scheduler time. The `num_cpus * 120` default is empirically tuned for
//!   high-throughput async workloads but might be excessive for CPU-bound actions.
use tokio::time::Instant;
use tokio::{sync::Notify, task::JoinHandle};
use typed_builder::TypedBuilder;

use crate::{aggregate::Aggregate, scenario::Scenario};

use futures::future::join_all;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
    u64,
};

pub trait Executor<A, F, Fut>
where
    Self: Send + Sync + Sized,
    A: Aggregate,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    /// Execute the scenario and return the final aggregate.
    fn exec(
        &self,
        scenario: &Scenario<A, Self, F, Fut>,
    ) -> impl Future<Output = Result<A, Box<dyn std::error::Error>>> + Send;
}

/// A stage defines a target RPS and how long to ramp to that target.
///
/// Use `Stage::new(Duration::from_secs(10), 100.0)` to ramp to 100 RPS over 10s.
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

/// Executor that drives a token-bucket governed by ramp stages and spawns worker tasks.
///
/// - Workers try to claim one token per request (CAS on `AtomicU64`).
/// - Governor ticks every `tick` and adds tokens according to the current interpolated rate.
/// - `bucket_capacity` bounds bursts (max surplus tokens stored from previous ticks).
#[derive(TypedBuilder)]
pub struct StageExecutor {
    pub stages: Vec<Stage>,
    #[builder(default = Duration::from_millis(100))]
    pub tick: Duration,
    #[builder(default = u64::MAX)]
    pub bucket_capacity: u64,
    // 120 workers per cpu seems like a good default number
    #[builder(default = num_cpus::get() * 120)]
    pub workers: usize,
}

impl<A, F, Fut> Executor<A, F, Fut> for StageExecutor
where
    Self: Send + Sync + Sized,
    A: Aggregate + 'static,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    async fn exec(
        &self,
        scenario: &Scenario<A, Self, F, Fut>,
    ) -> Result<A, Box<dyn std::error::Error>> {
        let start = Arc::new(Notify::new());
        let shutdown = Arc::new(AtomicBool::new(false));
        let tokens = Arc::new(AtomicU64::new(0));

        tracing::info!("Spawning token governor task...");
        let governor = tokio::spawn(token_governor_task(
            start.clone(),
            shutdown.clone(),
            tokens.clone(),
            self.stages.clone(),
            self.tick.clone(),
            self.bucket_capacity,
        ));

        tracing::info!("Spawning workers...");
        let handles = spawn_workers(
            self.workers,
            start.clone(),
            shutdown.clone(),
            tokens.clone(),
            scenario.action.clone(),
        )
        .await;

        tracing::info!("Running now!");
        start.notify_waiters();
        // The governor task ending means it's all over
        governor.await.expect("Error in token governor task");
        shutdown.store(true, Ordering::Relaxed);
        tracing::info!("Retrieving data from workers...");
        let aggs: Vec<A> = join_all(handles)
            .await
            .into_iter()
            .map(|res| res.expect("Task panicked"))
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

/// Governor task that increments the shared token counter according to stages.
pub async fn token_governor_task(
    start: Arc<Notify>,
    shutdown: Arc<AtomicBool>,
    tokens: Arc<AtomicU64>,
    stages: Vec<Stage>,
    tick: Duration,
    bucket_capacity: u64,
) {
    let mut rate = 0.0;
    let mut fractional = 0.0;

    for stage in stages.into_iter() {
        // wait until the benchmark has started
        start.notified().await;
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        // instantly jump to target rate
        // This technique make it possible to handle spikes or
        // start at a different rate in the same api
        if stage.duration.is_zero() {
            rate = stage.target;
            continue;
        }

        let stage_start = Instant::now();
        let start_rate = rate;
        let end_rate = stage.target;

        loop {
            let elapsed = Instant::now().duration_since(stage_start);
            if elapsed >= stage.duration {
                break;
            }

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
                // atomic saturating add with cas loop
                let mut prev = tokens.load(Ordering::Relaxed);
                loop {
                    let new = prev.saturating_add(add_total).min(bucket_capacity);
                    match tokens.compare_exchange(prev, new, Ordering::AcqRel, Ordering::Relaxed) {
                        Ok(_) => break,
                        Err(actual) => prev = actual,
                    }
                }
            }
            tokio::time::sleep(tick).await;
        }
        // Just to be sure the internal rate matches the stage target
        // so the next stage always start from the correct point and prevent
        // accumulating small rounding errors
        rate = end_rate;
    }
}

/// Pure function responsible for calculating the number of tokens to be added this tick
/// returning the total and the fractional
pub fn calc_token_limit(
    elapsed: Duration,
    stage_duration: Duration,
    start_rate: f64,
    end_rate: f64,
    fractional: f64,
    tick: Duration,
) -> (u64, f64) {
    // interpolation factor [0..1]
    let t = (elapsed.as_secs_f64() / stage_duration.as_secs_f64()).min(1.0);
    // linear interpolation
    let tick_rate = start_rate + (end_rate - start_rate) * t;
    // tokens to add this tick as float
    let add_f = tick_rate * tick.as_secs_f64();
    // convert float tokens to integer (carry the fractional part)
    let add_total = (add_f + fractional).floor() as u64;
    let fractional = (add_f + fractional) - (add_total as f64);
    (add_total, fractional)
}

/// Spawn `workers` Tokio tasks. Each worker claims tokens and executes the `action`.
pub async fn spawn_workers<A, F, Fut>(
    workers: usize,
    start: Arc<Notify>,
    shutdown: Arc<AtomicBool>,
    tokens: Arc<AtomicU64>,
    action: F,
) -> Vec<JoinHandle<A>>
where
    A: Aggregate + 'static,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    (0..workers)
        .map(|_| {
            let start = start.clone();
            let shutdown = shutdown.clone();
            let tokens = tokens.clone();
            let action = action.clone();
            tokio::spawn(async move {
                let mut agg = A::new();
                // Will hang if task is never started
                // I will handle it with a really safe shutdown
                // technique like racing futures or whatever
                start.notified().await;
                while !shutdown.load(Ordering::Relaxed) {
                    loop {
                        let cur = tokens.load(Ordering::Relaxed);
                        if cur == 0 {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                            if shutdown.load(Ordering::Relaxed) {
                                break;
                            }
                            continue;
                        }
                        if tokens.fetch_sub(1, Ordering::Relaxed) > 0 {
                            break;
                        }
                    }
                    if shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    let metric = action().await;
                    agg.consume(&metric);
                }
                agg
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Metric, macros::*};
    #[metric]
    struct EmptyMetric;

    #[aggregate]
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
        let start = Arc::new(Notify::new());
        let shutdown = Arc::new(AtomicBool::new(true));
        let tokens = Arc::new(AtomicU64::new(0));
        let action = || async { EmptyMetric {} };
        let workers: Vec<JoinHandle<EmptyAggregate>> =
            spawn_workers(n, start, shutdown, tokens, action).await;
        assert_eq!(workers.len(), n);
    }
}
