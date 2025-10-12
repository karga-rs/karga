use async_trait::async_trait;
use tokio::task::JoinHandle;
use tokio::time::Instant;
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

#[async_trait]
pub trait Executor<A, F, Fut>
where
    Self: Send + Sync + Sized,
    A: Aggregate,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    async fn exec(
        &self,
        scenario: &Scenario<A, Self, F, Fut>,
    ) -> Result<A, Box<dyn std::error::Error>>;
}

/// Controls the growth rate of the token bucket
///
/// Even if the bucket itself grows its up to the workers to consume it
///
/// The executor will always ramp towards the target, up or down
///
/// The growth rate will always be just enough to reach
/// the target
///
/// For example:
/// - `target: 100`, `duration: 10 secs` → grows at 10 per second, total around 500 requests
/// - For a sudden spike: use a large target over 0 seconds.
/// - For holding: use a spike and then ramp towards the same target.
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
/// - Workers try to claim one token per request (CAS on AtomicU64).
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

#[async_trait]
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
        let start = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let tokens = Arc::new(AtomicU64::new(0));
        let governor;

        println!("Spawning token governor task...");
        governor = tokio::spawn(token_governor_task(
            start.clone(),
            shutdown.clone(),
            tokens.clone(),
            self.stages.clone(),
            self.tick.clone(),
            self.bucket_capacity,
        ));

        println!("Spawning workers...");
        let handles = spawn_workers(
            self.workers,
            start.clone(),
            shutdown.clone(),
            tokens.clone(),
            scenario.action.clone(),
        )
        .await;

        println!("Running now!");
        start.store(true, Ordering::Release);
        // The governor task ending means its all over
        // billions must die
        governor.await.expect("Error in token governor task");
        shutdown.store(true, Ordering::Relaxed);
        println!("Retrieving data from workers...");
        let aggs: Vec<A> = join_all(handles)
            .await
            .into_iter()
            .map(|res| res.expect("Task panicked"))
            .collect();

        println!("Processing results...");
        let mut final_agg = A::new();
        for agg in aggs {
            final_agg.merge(agg);
        }
        println!("Done!");

        Ok(final_agg)
    }
}
pub async fn token_governor_task(
    start: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    tokens: Arc<AtomicU64>,
    stages: Vec<Stage>,
    tick: Duration,
    bucket_capacity: u64,
) {
    let mut rate = 0.0;
    let mut fractional = 0.0;

    for stage in stages.into_iter() {
        while !start.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
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
            let t = (elapsed.as_secs_f64() / stage.duration.as_secs_f64()).min(1.0);
            let tick_rate = start_rate + (end_rate - start_rate) * t;
            let add_f = tick_rate * tick.as_secs_f64();
            let add_total = (add_f + fractional).floor() as u64;
            fractional = (add_f + fractional) - (add_total as f64);
            if add_total > 0 {
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
        // Just to be sure
        rate = end_rate;
    }
}

pub async fn spawn_workers<A, F, Fut>(
    workers: usize,
    start: Arc<AtomicBool>,
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
                while !start.load(Ordering::Acquire) {
                    tokio::task::yield_now().await;
                }
                while !shutdown.load(Ordering::Relaxed) {
                    loop {
                        let cur = tokens.load(Ordering::Acquire);
                        if cur == 0 {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                            if shutdown.load(Ordering::Relaxed) {
                                break;
                            }
                            continue;
                        }
                        if tokens
                            .compare_exchange(cur, cur - 1, Ordering::AcqRel, Ordering::Relaxed)
                            .is_ok()
                        {
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
