use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{fmt::Debug, time::Duration};

use crate::Aggregate;

pub trait Report<A>
where
    Self: Send + Sync + Debug + From<A> + Serialize + DeserializeOwned,
    A: Aggregate,
{
}

#[async_trait]
pub trait Reporter<A: Aggregate, R: Report<A>> {
    async fn report(&self, report: R) -> Result<(), Box<dyn std::error::Error>>;
}

#[cfg(feature = "builtins")]
pub use builtins::*;

#[cfg(feature = "builtins")]
mod builtins {
    use crate::aggregate::BasicAggregate;

    use super::*;
    #[derive(Debug, Deserialize, Serialize)]
    pub struct BasicReport {
        pub average_latency: Duration,
        pub success_ratio: f64,
        pub total_bytes: usize,
        pub count: usize,
    }

    impl From<BasicAggregate> for BasicReport {
        fn from(value: BasicAggregate) -> Self {
            Self {
                average_latency: value.total_latency.div_f64(value.count as f64),
                success_ratio: (value.success_count as f64 / value.count as f64) * 100.0,
                total_bytes: value.total_bytes,
                count: value.count,
            }
        }
    }
    impl Report<BasicAggregate> for BasicReport {}

    pub struct StdoutReporter;
    #[async_trait]
    impl Reporter<BasicAggregate, BasicReport> for StdoutReporter {
        async fn report(&self, report: BasicReport) -> Result<(), Box<dyn std::error::Error>> {
            println!("{report:#?}");
            Ok(())
        }
    }
}
