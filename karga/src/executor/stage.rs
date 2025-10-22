use tokio::sync::Mutex;
use tokio::sync::watch::{self, Receiver};
use tokio::time::Instant;
use tokio::{sync::Notify, task::JoinHandle};
use typed_builder::TypedBuilder;

use super::Executor;
use crate::{aggregate::Aggregate, scenario::Scenario};
use internals::*;

use futures::future::join_all;
use std::{sync::Arc, time::Duration};

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
        let (ctx, shutdown_tx) = ExecutionContext::new();
        tracing::info!("Spawning token governor task...");
        let governor = tokio::spawn(token_governor_task(
            ctx.clone(),
            self.stages.clone(),
            self.tick,
            self.bucket_capacity,
        ));

        tracing::info!("Spawning workers...");
        let handles = spawn_workers(ctx.clone(), self.workers, scenario.action.clone()).await;

        tracing::info!("Running now!");
        ctx.start.notify_waiters();
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
    use tokio::sync::{Semaphore, watch::Sender};

    use super::*;

    #[derive(Clone)]
    pub struct ExecutionContext {
        pub start: Arc<Notify>,
        pub shutdown: Receiver<bool>,
        pub tokens: Arc<Semaphore>,
    }

    impl ExecutionContext {
        pub fn new() -> (Self, Sender<bool>) {
            let (tx, rx) = watch::channel(false);
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

    /// Governor task that increments the shared token counter according to stages.
    pub async fn token_governor_task(
        mut ctx: ExecutionContext,
        stages: Vec<Stage>,
        tick: Duration,
        bucket_capacity: u64,
    ) {
        let main_task = || async {
            let mut rate = 0.0;
            let mut fractional = 0.0;
            // wait until the benchmark has started
            ctx.start.notified().await;

            for stage in stages.into_iter() {
                // instantly jump to target rate
                // This technique make it possible to handle spikes or
                // start at a different rate in the same api
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
                    next_tick += tick;
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
                        let avail = ctx.tokens.available_permits() as u64;
                        if avail < bucket_capacity {
                            let free_cap = bucket_capacity - avail;
                            let add = add_total.min(free_cap) as usize;
                            println!("{avail}:{add}");
                            if add > 0 {
                                ctx.tokens.add_permits(add);
                            }
                        }
                    }
                    tokio::time::sleep_until(next_tick).await;
                }
                // Just to be sure the internal rate matches the stage target
                // so the next stage always start from the correct point and prevent
                // accumulating small rounding errors
                rate = end_rate;
            }
        };

        tokio::select! {
            _ = main_task() =>{}
            _ = ctx.shutdown.wait_for(|b|*b) => {}

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
            .map(|_| {
                let mut ctx = ctx.clone();
                let action = action.clone();
                tokio::spawn(async move {
                    let agg = Arc::new(Mutex::new(A::new()));

                    let main_task = || async {
                        let agg = agg.clone();
                        ctx.start.notified().await;
                        loop {
                            let permit = ctx.tokens.clone().acquire_owned().await;

                            match permit {
                                Ok(p) => {
                                    p.forget();
                                    let metric = action().await;
                                    agg.lock().await.consume(&metric);
                                }
                                Err(_) => break,
                            }
                        }
                    };

                    tokio::select! {
                        _ = main_task() =>{}
                        _ = ctx.shutdown.wait_for(|b|*b) => {}

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
        let (ctx, _) = ExecutionContext::new();
        let action = || async { EmptyMetric {} };
        let workers: Vec<JoinHandle<EmptyAggregate>> = spawn_workers(ctx, n, action).await;

        assert_eq!(workers.len(), n);
    }
}
