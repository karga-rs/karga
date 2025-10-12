use std::fmt::Debug;

use serde::{Serialize, de::DeserializeOwned};

/// Metrics that should be collected and processed by the framework
/// Metrics can be composed of other metrics as well
pub trait Metric
where
    Self: Serialize + DeserializeOwned + PartialOrd + PartialEq + Send + Sync + Debug + Clone,
{
}

#[cfg(feature = "builtins")]
pub use builtins::*;

#[cfg(feature = "builtins")]
mod builtins {
    use crate::macros::metric;
    use std::time::Duration;

    use super::*;

    #[metric]
    pub struct BasicMetric {
        pub latency: Duration,
        pub success: bool,
        pub bytes: usize,
    }
}
