use std::fmt::Debug;

use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::mpsc;

/// Metrics that should be collected and processed by the framework
/// Metrics can be composed of other metrics as well
pub trait Metric
where
    Self: Serialize + DeserializeOwned + PartialOrd + PartialEq + Send + Sync + Default + Debug,
{
}

pub trait Aggregator
where
    Self: Send + Sync + Debug,
{
    type Metric: Metric;
    fn new() -> Self;
    /// Aggregate metrics into itself
    fn aggregate(&mut self, metrics: &[Self::Metric]);
    /// COmbine two different aggregates into one
    fn combine(&self, other: Self) -> Self;
}

/// Tokio task for efficient metric aggregation
pub(crate) async fn aggregator_task<A: Aggregator>(
    mut rx: mpsc::Receiver<A::Metric>,
    batch_size: usize,
) -> A {
    let mut agg = A::new();
    let mut batch = Vec::new();

    loop {
        // Receive the first metric or end the loop if the sender is dropped
        match rx.recv().await {
            Some(metric) => batch.push(metric),
            None => break,
        }

        // Receive all other metrics after the first one
        while batch.len() < batch_size {
            match rx.try_recv() {
                Ok(metric) => batch.push(metric),
                Err(_) => break,
            }
        }

        // Imediately aggregate any available metrics
        if !batch.is_empty() {
            agg.aggregate(&batch);
            batch.clear();
        }
    }
    agg
}
