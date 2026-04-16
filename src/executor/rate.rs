//! The `RateExecutor` and its components, providing a high-precision,
//! stage-based execution model.
//!
//! The `RateExecutor` implements a token-bucket strategy based on the
//! mathematical integral of the RPS curve defined by a list of [`Stage`]s.
//! Each `Stage` defines a target requests-per-second (RPS) and a duration,
//! and the executor smoothly interpolates the rate between stages.
//!
//! This executor presents high-performance with **zero drift** while being
//! completely **lock-free**

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

use tokio::{sync::Semaphore, task::JoinHandle, time::Instant};
use typed_builder::TypedBuilder;

/// A stage defines a target RPS and how long to ramp to that target.
///
/// Use `Stage::new(Duration::from_secs(10), 100.0)` to ramp to 100 RPS over 10s.
/// For a spike in rate, use a very small duration like `Duration::from_nanos(1)`.
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
const NANOS_PER_SEC: f64 = 1_000_000_000.0;

/// A pre-calculated representation of a rate-limiting stage for fast lookup
#[derive(Debug)]
struct InternalStage {
    abs_start_ns: u64,
    abs_end_ns: u64,
    start_rate: f64,
    end_rate: f64,
}

impl InternalStage {
    // Here is where the magic happens
    // it is basically an integral, it says the ammount of tokens that there should be
    // at any point in time
    //
    // It is an integral, so it only calculates the area under the curve
    // not the ammount of tokens that should be added or the rate of change, which
    // simplifies its logic
    fn tokens_at(&self, now: u64) -> f64 {
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

/// The [`RateLimiter`] is responsible for controlling the minting of tokens
/// which workers depend on to perform any work, handling of batches and burst control.
///
/// It is **lock-free** and works on the **pull model**.
#[derive(Debug)]
struct RateLimiter {
    tokens: Semaphore,
    stages: Vec<InternalStage>,
    total_duration: Duration,
    start: Instant,
    /// Marks how many tokens were minted up to this point
    /// combined with tokens_at we can get how many tokens must be added to fill the gap
    tokens_minted: AtomicU64,
    max_burst: f64,
    max_batch: u64,
    workers: usize,
    batch: AtomicU64,
}

impl RateLimiter {
    fn new(
        stages: &[Stage],
        max_burst: f64,
        max_batch: u64,
        start_rate: f64,
        workers: usize,
    ) -> Self {
        let now = Instant::now();
        RateLimiter {
            tokens: Semaphore::new(0),
            stages: Self::stages_to_internal(stages, start_rate),
            total_duration: Self::total_duration(stages),
            start: now,
            tokens_minted: AtomicU64::new(0),
            max_burst,
            max_batch,
            workers,
            batch: AtomicU64::new(1),
        }
    }

    pub async fn acquire(&self) -> Option<u32> {
        loop {
            let now = Instant::now().duration_since(self.start);

            if now > self.total_duration {
                self.tokens.close();
                return None;
            };

            let mut batch = self.batch.load(Ordering::Relaxed) as u32;
            if let Ok(p) = self.tokens.try_acquire_many(batch) {
                p.forget();
                return Some(batch);
            }

            self.refill(now.as_nanos() as u64);
            // we load again because the size of the batch might have changed after the refill
            batch = self.batch.load(Ordering::Relaxed) as u32;
            match tokio::time::timeout(Duration::from_millis(100), self.tokens.acquire_many(batch))
                .await
            {
                Ok(Ok(p)) => {
                    p.forget();
                    return Some(batch);
                }
                _ => continue,
            };
        }
    }

    // the refilling currently works based on the assumption that no matters who
    // succeeds in refilling the bucket and at which time, it will always be following
    // the integral at that point in time, and any imprecisions will be accounted for
    // in the next refill
    fn refill(&self, now: u64) {
        let expected = self.total_tokens_at(now);
        let minted = self.tokens_minted.load(Ordering::Acquire);
        if expected as u64 > minted {
            // fill the gap between the math and reality
            let add = (expected as u64 - minted).min(self.max_burst as u64);
            if self
                .tokens_minted
                .compare_exchange(minted, minted + add, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                self.tokens.add_permits(MAX_TOKENS.min(add as usize));
            };
            let batch = ((add / self.workers as u64 / 50)
                .min(self.max_burst as u64)
                .min(self.max_batch))
            .max(1);
            self.batch.store(batch, Ordering::Release);
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

    fn stages_to_internal(stages: &[Stage], start_rate: f64) -> Vec<InternalStage> {
        let mut internals = Vec::with_capacity(stages.len());
        let (mut last_abs_end, mut last_rate_end) = (0, start_rate);
        for s in stages.iter() {
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

/// Executor that drives a high-precision, integral-based rate control system.
///
/// The `RateExecutor` uses a mathematical integral of the RPS curve to ensure
/// nanosecond-perfect rate adherence across any number of stages.
///
/// # Example
///
/// ```rust
/// use std::time::Duration;
/// use karga::{RateExecutor, Stage};
///
/// let executor = RateExecutor::builder()
///     .workers(10)
///     .stages(vec![
///         Stage::new(Duration::from_secs(5), 100.0), // Ramp to 100 RPS
///         Stage::new(Duration::from_secs(10), 100.0), // Hold 100 RPS
///     ])
///     .build();
/// ```
#[derive(TypedBuilder)]
pub struct RateExecutor {
    /// The sequence of rate-control stages to execute.
    pub stages: Vec<Stage>,
    /// The number of concurrent worker tasks to spawn.
    pub workers: usize,
    /// The maximum number of tokens that can be minted at once to catch up if the executor falls behind.
    #[builder(default = f64::MAX)]
    pub max_burst: f64,
    /// The starting RPS for the first stage.
    #[builder(default = 0.)]
    pub start_rate: f64,

    /// Batch sizes are calculated automatically from the rate, but if you need control over batch sizes
    /// in situations like tests with target at infinity, this has you covered.
    ///
    /// To disable batching, set this to 1.
    /// Defaults to 20.
    #[builder(default = 20)]
    pub max_batch: u64,
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
        let rlt = Arc::new(RateLimiter::new(
            &self.stages,
            self.max_burst,
            self.max_batch,
            self.start_rate,
            self.workers,
        ));

        tracing::info!("Spawning {} workers...", self.workers);
        let handles = spawn_workers(rlt.clone(), self.workers, scenario.action.clone()).await;

        tracing::info!("Running scenario: {}!", scenario.name);

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

/// Spawns `workers` Tokio tasks, each acting as a test worker.
///
/// Each worker waits for the start signal, then enters a loop to
/// acquire tokens and execute the `action`.
async fn spawn_workers<A, F, Fut>(
    rlt: Arc<RateLimiter>,
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
            let rlt = rlt.clone();
            let action = action.clone();
            tokio::spawn(async move {
                let mut agg = A::new();
                tracing::debug!("Worker {i} spawned.");
                loop {
                    let permits = match rlt.acquire().await {
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
        macro_rules! rlm {
            ($s:ident) => {
                RateLimiter::new(&$s, f64::MAX, 1, 0., 1)
            };
        }

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
            let rl = rlm!(sts);
            assert_eq!(rl.total_duration, Duration::from_millis(60))
        }
        #[test]
        fn double_refill() {
            let sts = vec![Stage {
                duration: Duration::from_secs(10),
                target: 100.,
            }];
            let rl = rlm!(sts);
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
            let rl = rlm!(sts);
            assert_eq!(rl.acquire().await, None);
        }

        #[test]
        fn batching() {
            let sts = vec![Stage {
                duration: Duration::from_secs(1),
                target: 500.,
            }];

            let rl = RateLimiter::new(&sts, 500., 500, 500., 2);
            let batch = rl.batch.load(Ordering::Relaxed);
            assert_eq!(batch, 1);
            rl.refill(Duration::from_secs(1).as_nanos() as u64);
            let batch = rl.batch.load(Ordering::Relaxed);
            // given that we have 2 workers and each need to hit 50 times a second
            assert_eq!(batch, 5);
        }
        #[test]
        fn max_batch() {
            let sts = vec![Stage {
                duration: Duration::from_secs(1),
                target: f64::MAX,
            }];
            let rl = RateLimiter::new(&sts, f64::MAX, 10, f64::MAX, 1);
            let batch = rl.batch.load(Ordering::Relaxed);
            assert_eq!(batch, 1);
            rl.refill(Duration::from_secs(1).as_nanos() as u64);
            let batch = rl.batch.load(Ordering::Relaxed);
            assert_eq!(batch, 10);
        }
    }
}
