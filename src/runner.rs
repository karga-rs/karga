use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::sync::mpsc;

use crate::{
    metrics::{Aggregator, Metric, aggregator_task},
    scenario::Scenario,
};

pub enum Stage {
    RampUp,
    Hold,
    RampDown,
}
pub struct Runner<A, T, F, Fut>
where
    A: Aggregator,
    T: Metric,
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = T> + Send,
{
    pub scenarios: Vec<Scenario<T, F, Fut>>,
    pub aggregator: A,
}

impl<A, T, F, Fut> Runner<A, T, F, Fut>
where
    A: Aggregator + 'static,
    T: Metric + 'static,
    F: Fn() -> Fut + Send + Sync + Copy + Clone + 'static,
    Fut: Future<Output = T> + Send,
{
    pub fn new() -> Self {
        Self {
            scenarios: vec![],
            aggregator: A::new(),
        }
    }

    pub fn add_scenario(&mut self, scenario: Scenario<T, F, Fut>) -> &mut Self {
        self.scenarios.push(scenario);
        self
    }

    pub async fn constant_vus(&self, vus: usize, duration: Duration) {
        for scenario in &self.scenarios {
            let (results_tx, results_rx) = mpsc::channel(vus * 10);
            let shutdown_signal = Arc::new(AtomicBool::new(false));

            let aggregator_handle = tokio::spawn(aggregator_task::<A, T>(results_rx, vus * 10));

            for _ in 0..vus {
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
            tokio::time::sleep(duration).await;
            shutdown_signal.store(true, Ordering::Relaxed);

            let final_aggregator = aggregator_handle.await.unwrap();
            self.aggregator = self.aggregator.combine(final_aggregator);
        }
    }
}
