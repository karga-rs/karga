use async_trait::async_trait;

use crate::{metrics::Aggregate, scenario::Scenario};

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

#[cfg(feature = "builtins")]
pub use builtins::*;

#[cfg(feature = "builtins")]
mod builtins {
    use super::*;

    use futures::future::join_all;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    pub struct ConstantExecutor {
        duration: Duration,
        workers: usize,
    }

    impl ConstantExecutor {
        pub fn new(duration: Duration, workers_per_cpu: usize) -> Self {
            let workers = num_cpus::get() * workers_per_cpu;
            Self { duration, workers }
        }
    }
    #[async_trait]
    impl<A, F, Fut> Executor<A, F, Fut> for ConstantExecutor
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
            let shutdown_signal = Arc::new(AtomicBool::new(false));
            let mut handles = vec![];

            for _ in 0..self.workers {
                let action = scenario.action.clone();
                let shutdown = Arc::clone(&shutdown_signal);

                handles.push(tokio::spawn(async move {
                    let mut agg = A::new();
                    while !shutdown.load(Ordering::Relaxed) {
                        let metric = action().await;
                        agg.consume(&metric);
                    }
                    agg
                }));
            }
            tokio::time::sleep(self.duration).await;
            shutdown_signal.store(true, Ordering::Relaxed);
            let aggs: Vec<A> = join_all(handles)
                .await
                .into_iter()
                .map(|res| res.expect("Task panicked"))
                .collect();

            let mut final_agg = A::new();
            for agg in aggs {
                final_agg.merge(agg);
            }

            Ok(final_agg)
        }
    }
}
