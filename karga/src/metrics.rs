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
    fn aggregate(&mut self, metrics: &[Self::Metric]) {
        metrics.iter().for_each(|m| self.consume(m));
    }
    /// Aggregate a single metric into itself
    fn consume(&mut self, metric: &Self::Metric);
    /// Combine two different aggregates into one
    fn merge(&mut self, other: Self);
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

        fn consume(&mut self, metric: &Self::Metric) {
            self.total_latency += metric.latency;
            self.success_count += if metric.success { 1 } else { 0 };
            self.total_bytes += metric.bytes;
            self.count += 1;
        }

        fn merge(&mut self, other: Self) {
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
