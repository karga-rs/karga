use tokio::sync::Mutex;
use tokio::sync::watch::{self, Receiver};
use tokio::time::Instant;
use tokio::{sync::Notify, task::JoinHandle};
use typed_builder::TypedBuilder;

use super::Executor;
use crate::{aggregate::Aggregate, scenario::Scenario};
use internals::*;

use futures::future::join_all;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
    u64,
};

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
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let tokens = Arc::new(AtomicU64::new(0));
        let tokens_added = Arc::new(Notify::new());

        tracing::info!("Spawning token governor task...");
        let governor = tokio::spawn(token_governor_task(
            start.clone(),
            shutdown_rx.clone(),
            tokens.clone(),
            tokens_added.clone(),
            self.stages.clone(),
            self.tick.clone(),
            self.bucket_capacity,
        ));

        tracing::info!("Spawning workers...");
        let handles = spawn_workers(
            self.workers,
            start.clone(),
            shutdown_rx.clone(),
            tokens.clone(),
            tokens_added.clone(),
            scenario.action.clone(),
        )
        .await;

        tracing::info!("Running now!");
        start.notify_waiters();
        // The governor task ending means it's all over
        governor.await.expect("Error in token governor task");
        shutdown_tx.send(true)?;
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

#[cfg(feature = "internals")]
pub use internals::*;

mod internals {
    use super::*;
    /// Governor task that increments the shared token counter according to stages.
    pub async fn token_governor_task(
        start: Arc<Notify>,
        mut shutdown: Receiver<bool>,
        tokens: Arc<AtomicU64>,
        tokens_added: Arc<Notify>,
        stages: Vec<Stage>,
        tick: Duration,
        bucket_capacity: u64,
    ) {
        let main_task = || async {
            let mut rate = 0.0;
            let mut fractional = 0.0;

            for stage in stages.into_iter() {
                // wait until the benchmark has started
                start.notified().await;
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
                            match tokens.compare_exchange(
                                prev,
                                new,
                                Ordering::AcqRel,
                                Ordering::Relaxed,
                            ) {
                                Ok(_) => {
                                    tokens_added.notify_waiters();
                                    break;
                                }
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
        };

        tokio::select! {
            _ = main_task() =>{}
            _ = shutdown.wait_for(|b|*b) => {}

        };
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
        shutdown: Receiver<bool>,
        tokens: Arc<AtomicU64>,
        tokens_added: Arc<Notify>,
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
                let mut shutdown = shutdown.clone();
                let tokens = tokens.clone();
                let tokens_added = tokens_added.clone();
                let action = action.clone();
                tokio::spawn(async move {
                    let agg = Arc::new(Mutex::new(A::new()));
                    let main_task = || async {
                        let agg = agg.clone();
                        start.notified().await;
                        loop {
                            loop {
                                let cur = tokens.load(Ordering::Relaxed);
                                if cur == 0 {
                                    tokens_added.notified().await;
                                    continue;
                                }
                                if tokens.fetch_sub(1, Ordering::Relaxed) > 0 {
                                    break;
                                }
                            }
                            let metric = action().await;
                            agg.lock().await.consume(&metric);
                        }
                    };
                    tokio::select! {
                        _ = main_task() =>{}
                        _ = shutdown.wait_for(|b|*b) => {}

                    };

                    // Shouldnt usually be a problem, if it is i fucked up really bad im sorry
                    Arc::try_unwrap(agg)
                        .expect("Arc::try_unwrap on aggregator failed")
                        .into_inner()
                })
            })
            .collect()
    }
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
        let (_, shutdown) = watch::channel(true);
        let tokens = Arc::new(AtomicU64::new(0));
        let tokens_added = Arc::new(Notify::new());
        let action = || async { EmptyMetric {} };
        let workers: Vec<JoinHandle<EmptyAggregate>> =
            spawn_workers(n, start, shutdown, tokens, tokens_added, action).await;

        assert_eq!(workers.len(), n);
    }
}
