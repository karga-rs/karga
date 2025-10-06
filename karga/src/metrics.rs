use std::fmt::Debug;

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::mpsc;

/// Metrics that should be collected and processed by the framework
/// Metrics can be composed of other metrics as well
pub trait Metric
where
    Self: Serialize + DeserializeOwned + PartialOrd + PartialEq + Send + Sync + Debug + Clone,
{
}

pub trait Aggregate
where
    Self: Serialize + DeserializeOwned + PartialOrd + PartialEq + Send + Sync + Debug + Clone,
{
    type Metric: Metric;
    fn new() -> Self;
    /// Aggregate metrics into itself
    fn aggregate(&mut self, metrics: &[Self::Metric]);
    /// Combine two different aggregates into one
    fn combine(&mut self, other: Self);
}

/// Tokio task for efficient metric aggregation
pub async fn aggregator_task<A: Aggregate>(
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

/// Reporters are responsible for taking an aggregate and serializing it into json and print to the screen
/// or send to some service via protobuff, or whatever and however you want it to do. More power to you
#[async_trait]
pub trait Report<A>
where
    Self: Send + Sync + Debug + From<A> + Serialize + DeserializeOwned,
    A: Aggregate,
{
    async fn report(&self) -> Result<(), Box<dyn std::error::Error>>;
}

#[cfg(feature = "builtins")]
pub use builtins::*;

#[cfg(feature = "builtins")]
mod builtins {
    use karga_macros::aggregate;
    use serde::Deserialize;

    use crate::macros::metric;
    use std::time::Duration;

    use super::*;

    #[metric]
    pub struct BasicMetric {
        pub latency: Duration,
        pub success: bool,
        pub bytes: usize,
    }

    #[aggregate]
    #[derive(Default)]
    pub struct BasicAggregate {
        pub total_latency: Duration,
        pub success_count: usize,
        pub total_bytes: usize,
        pub count: usize,
    }

    impl Aggregate for BasicAggregate {
        type Metric = BasicMetric;

        fn new() -> Self {
            BasicAggregate::default()
        }

        fn aggregate(&mut self, metrics: &[Self::Metric]) {
            for m in metrics {
                self.total_latency += m.latency;
                self.success_count += if m.success { 1 } else { 0 };
                self.total_bytes += self.total_bytes;
                self.count += 1;
            }
        }

        fn combine(&mut self, other: Self) {
            self.total_latency += other.total_latency;
            self.success_count += other.success_count;
            self.total_bytes += other.total_bytes;
            self.count += other.count;
        }
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct JsonReport {
        pub average_latency: Duration,
        pub success_ratio: u8,
        pub total_bytes: usize,
        pub count: usize,
    }

    impl From<BasicAggregate> for JsonReport {
        fn from(value: BasicAggregate) -> Self {
            Self {
                average_latency: value.total_latency.div_f64(value.count as f64),
                success_ratio: ((value.success_count / value.count) * 100)
                    .try_into()
                    .unwrap(),
                total_bytes: value.total_bytes,
                count: value.count,
            }
        }
    }
    #[async_trait]
    impl Report<BasicAggregate> for JsonReport {
        async fn report(&self) -> Result<(), Box<dyn std::error::Error>> {
            let value = serde_json::to_string(self)?;
            println!("{value}");
            Ok(())
        }
    }
}
