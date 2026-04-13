//! The `StageExecutor` and its components, providing a rate-controlled,
//! stage-based execution model.
//!
//! The `StageExecutor` implements a token-bucket governor driven by a list of
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
use super::Executor;
use crate::{aggregate::Aggregate, scenario::Scenario};
use futures::future::join_all;
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    sync::{Notify, Semaphore},
    task::JoinHandle,
    time::Instant,
};
use typed_builder::TypedBuilder;

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
#[allow(dead_code)]
const MAX_TOKENS: usize = usize::MAX >> 3;
const NANOS_PER_SEC: f64 = 1_000_000_000.0;

struct InternalStage {
    abs_start_ns: u64,
    abs_end_ns: u64,
    start_rate: f64,
    end_rate: f64,
}
impl InternalStage {
    fn tokens_at(&self, now: u64) -> f64 {
        // duration 0
        if self.abs_start_ns == self.abs_end_ns {
            return self.end_rate;
        };
        let total_secs = (self.abs_end_ns - self.abs_start_ns) as f64 / NANOS_PER_SEC;
        let slider = (now.clamp(self.abs_start_ns, self.abs_end_ns) - self.abs_start_ns) as f64
            / NANOS_PER_SEC;

        let (rend, rst) = (self.end_rate, self.start_rate);
        let base_tokens = rst * slider;
        let slope = 0.5 * (rend - rst) * (slider * slider / total_secs);
        base_tokens + slope
    }

    fn total_area(&self) -> f64 {
        self.tokens_at(self.abs_end_ns)
    }
}

struct RateLimiter {
    tokens: Semaphore,
    stages: Vec<InternalStage>,
    total_duration: Duration,
    start: Instant,
    tokens_minted: AtomicU64,
}

impl RateLimiter {
    fn new(stages: &[Stage]) -> Self {
        let now = Instant::now();
        RateLimiter {
            tokens: Semaphore::new(0),
            stages: Self::stages_to_internal(stages),
            total_duration: Self::total_duration(stages),
            start: now,
            tokens_minted: AtomicU64::new(0),
        }
    }

    pub async fn acquire(&self) -> Option<u32> {
        loop {
            let now = Instant::now().duration_since(self.start);
            if now > self.total_duration {
                self.tokens.close();
                return None;
            };

            if let Ok(p) = self.tokens.try_acquire() {
                p.forget();
                return Some(1);
            }

            self.refill(now.as_nanos() as u64);
            match tokio::time::timeout(Duration::from_millis(100), self.tokens.acquire()).await {
                Ok(Ok(p)) => {
                    p.forget();
                    return Some(1);
                }
                _ => continue,
            };
        }
    }

    fn refill(&self, now: u64) {
        let expected = self.total_tokens_at(now);
        let minted = self.tokens_minted.load(Ordering::Acquire);
        if expected as u64 > minted {
            let add = expected as u64 - minted;
            if self
                .tokens_minted
                .compare_exchange(minted, minted + add, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                self.tokens.add_permits(add as usize);
            };
        };
    }

    fn total_tokens_at(&self, now: u64) -> f64 {
        let mut total = 0.0;
        for stage in &self.stages {
            if now >= stage.abs_end_ns {
                total += stage.total_area();
            } else if now > stage.abs_start_ns {
                total += stage.tokens_at(now);
                break;
            } else {
                break;
            }
        }
        total
    }

    fn total_duration(stages: &[Stage]) -> Duration {
        stages.iter().map(|s| s.duration).sum()
    }

    fn stages_to_internal(stages: &[Stage]) -> Vec<InternalStage> {
        let mut internals = Vec::with_capacity(stages.len());
        // gambiarra moment
        let first = stages.first().unwrap_or(&Stage {
            duration: Duration::ZERO,
            target: 0.,
        });

        let (mut last_abs_end, mut last_rate_end) =
            (first.duration.as_nanos() as u64, first.target);

        internals.push(InternalStage {
            abs_start_ns: 0,
            abs_end_ns: first.duration.as_nanos() as u64,
            start_rate: 0.,
            end_rate: first.target,
        });
        for s in stages.iter().skip(1) {
            internals.push(InternalStage {
                abs_start_ns: last_abs_end,
                abs_end_ns: last_abs_end + s.duration.as_nanos() as u64,
                start_rate: last_rate_end,
                end_rate: s.target,
            });
            last_abs_end += s.duration.as_nanos() as u64;
            last_rate_end = s.target
        }
        internals
    }
}

/// Executor that drives a token-bucket governed by ramp stages and spawns worker tasks.
///
/// This executor implements a token-bucket rate-limiting strategy using a
/// [`tokio::sync::Semaphore`].
///
/// - A pool of `workers` tasks is spawned. Each worker asynchronously waits to
///   acquire a permit from the semaphore.
/// - Once a permit is acquired, the worker "forgets" it (preventing it from being
///   returned) and executes the `action`.
#[derive(TypedBuilder)]
pub struct RateExecutor {
    /// The sequence of rate-control stages to execute.
    pub stages: Vec<Stage>,
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
        let ctx = ExecutionContext::new(&self.stages);

        tracing::info!("Spawning {} workers...", self.workers);
        let handles = spawn_workers(ctx.clone(), self.workers, scenario.action.clone()).await;

        tracing::info!("Running scenario: {}!", scenario.name);
        ctx.start.notify_waiters();

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

/// Shared execution state for the governor and all worker tasks.
#[derive(Clone)]
struct ExecutionContext {
    /// Broadcasts the signal to start the test.
    start: Arc<Notify>,
    tokens: Arc<RateLimiter>,
}

impl ExecutionContext {
    fn new(stages: &[Stage]) -> Self {
        Self {
            start: Arc::new(Notify::new()),
            tokens: Arc::new(RateLimiter::new(stages)),
        }
    }
}

/// Spawns `workers` Tokio tasks, each acting as a test worker.
///
/// Each worker waits for the start signal, then enters a loop to
/// acquire tokens and execute the `action`.
async fn spawn_workers<A, F, Fut>(
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
            let ctx = ctx.clone();
            let action = action.clone();
            tokio::spawn(async move {
                let mut agg = A::new();
                tracing::debug!("Worker {i} spawned.");

                ctx.start.notified().await;
                tracing::debug!("Worker {i} started.");

                loop {
                    let permits = match ctx.tokens.acquire().await {
                        Some(p) => p,
                        None => {
                            tracing::debug!(
                                "Worker {i} failed to acquire token (semaphore closed).",
                            );
                            break;
                        }
                    };

                    for _ in 0..permits {
                        let metric = action().await;
                        agg.consume(&metric);
                    }
                }

                tracing::debug!("Worker {i} shutting down.",);
                agg
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    mod internal_stage {
        const TDUR: u64 = 10 * NANOS_PER_SEC as u64;
        macro_rules! abcd {
            ($s:ident) => {
                (
                    $s.tokens_at(NANOS_PER_SEC as u64),
                    $s.tokens_at(2 * NANOS_PER_SEC as u64),
                    $s.tokens_at(5 * NANOS_PER_SEC as u64),
                    $s.tokens_at(10 * NANOS_PER_SEC as u64),
                )
            };
        }

        use super::*;
        #[test]
        fn ramp_down() {
            let start_rate = 100.;
            let end_rate = 0.;
            let s = InternalStage {
                abs_start_ns: 0,
                abs_end_ns: TDUR,
                start_rate,
                end_rate,
            };
            let (a, b, c, d) = abcd!(s);
            // according
            assert_eq!(a, 95.);
            assert_eq!(b, 180.);
            assert_eq!(c, 375.);
            assert_eq!(d, 500.);
        }
        #[test]
        fn hold_steady() {
            let start_rate = 100.;
            let end_rate = 100.;
            let s = InternalStage {
                abs_start_ns: 0,
                abs_end_ns: TDUR,
                start_rate,
                end_rate,
            };
            let (a, b, c, d) = abcd!(s);
            assert_eq!(a, 100.);
            assert_eq!(b, 200.);
            assert_eq!(c, 500.);
            assert_eq!(d, 1000.)
        }

        #[test]
        fn ramp_up() {
            let start_rate = 0.;
            let end_rate = 100.;
            let s = InternalStage {
                abs_start_ns: 0,
                abs_end_ns: TDUR,
                start_rate,
                end_rate,
            };

            let (a, b, c, d) = abcd!(s);
            assert_eq!(a, 5.);
            assert_eq!(b, 20.);
            assert_eq!(c, 125.);
            assert_eq!(d, 500.)
        }

        #[test]
        fn instant_jump() {
            let s = InternalStage {
                abs_start_ns: 0,
                abs_end_ns: 0,
                start_rate: 0.,
                end_rate: 100.,
            };
            let tokens = s.tokens_at(0);
            assert_eq!(tokens, s.end_rate)
        }

        #[test]
        fn uncapped() {
            let s = InternalStage {
                abs_start_ns: 0,
                abs_end_ns: TDUR,
                start_rate: f64::MAX,
                end_rate: f64::MAX,
            };

            assert_eq!(s.total_area(), f64::INFINITY);
        }
    }

    mod rate_limiter {
        use std::{sync::atomic::Ordering, time::Duration};

        use crate::{executor::rate::RateLimiter, Stage};

        #[test]
        fn test_over() {
            let sts = vec![
                Stage {
                    duration: Duration::from_millis(10),
                    target: 100.,
                },
                Stage {
                    duration: Duration::from_millis(50),
                    target: 100.,
                },
            ];
            let rl = RateLimiter::new(&sts);
            assert_eq!(rl.total_duration, Duration::from_millis(60))
        }
        #[test]
        fn double_refill() {
            let sts = vec![Stage {
                duration: Duration::from_secs(10),
                target: 100.,
            }];
            let rl = RateLimiter::new(&sts);
            let now = Duration::from_secs(5).as_nanos() as u64;
            rl.refill(now);
            let mnt = rl.tokens_minted.load(Ordering::Relaxed);

            rl.refill(now);
            let mnt2 = rl.tokens_minted.load(Ordering::Relaxed);

            assert_eq!(mnt, mnt2);
            assert_eq!(mnt, 125);
        }

        #[tokio::test]
        async fn end_test() {
            let sts = vec![Stage {
                duration: Duration::from_nanos(1),
                target: 1.,
            }];
            let rl = RateLimiter::new(&sts);
            assert_eq!(rl.acquire().await, None);
        }
    }
}
