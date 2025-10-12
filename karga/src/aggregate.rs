use std::fmt::Debug;

use crate::Metric;
use serde::{Serialize, de::DeserializeOwned};
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

#[cfg(feature = "builtins")]
pub use builtins::*;

#[cfg(feature = "builtins")]
mod builtins {
    use std::time::Duration;

    use crate::metric::BasicMetric;

    use super::*;
    use karga_macros::aggregate;
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
}
