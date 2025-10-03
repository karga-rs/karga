use async_trait::async_trait;

use crate::{
    metrics::{Aggregate},
    scenario::Scenario,
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

#[cfg(feature = "builtins")]
pub use builtins::*;

#[cfg(feature = "builtins")]
mod builtins {
    use super::*;
    use tokio::sync::mpsc;

    use crate::metrics::aggregator_task;

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
            let (results_tx, results_rx) = mpsc::channel(self.workers * 10);
            let shutdown_signal = Arc::new(AtomicBool::new(false));

            let aggregator_handle =
                tokio::spawn(aggregator_task::<A>(results_rx, self.workers * 10));

            for _ in 0..self.workers {
                let action = scenario.action.clone();
                let tx = results_tx.clone();
                let shutdown = Arc::clone(&shutdown_signal);

                tokio::spawn(async move {
                    while !shutdown.load(Ordering::Relaxed) {
                        let metric = action().await;
                        if tx.send(metric).await.is_err() {
                            break;
                        }
                    }
                });
            }
            drop(results_tx);
            tokio::time::sleep(self.duration).await;
            shutdown_signal.store(true, Ordering::Relaxed);

            let final_aggregator = aggregator_handle.await.unwrap();
            Ok(final_aggregator)
        }
    }
}
