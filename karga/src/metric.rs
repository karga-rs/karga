use std::fmt::Debug;

/// A `Metric` represents a single observed measurement produced by the system under test.
///
/// Metrics are the most granular level of performance or behavioral data. They may capture
/// latency, success/failure, throughput, resource usage, or any other quantitative aspect
/// of an operation. Metrics are later collected and summarized by an [`crate::Aggregate`], then
/// further analyzed and reported by a [`crate::Report`] and [`crate::Reporter`].
///
/// ## Design principles
/// - **Simple and composable:** metrics should be lightweight and may be composed of other
///   metrics. For example, a `BasicMetric` might measure latency and success, while a more
///   advanced metric could embed multiple sub-metrics (network, CPU, I/O, etc.).
/// - **Comparable:** metrics must support [`PartialEq`] and [`PartialOrd`] to enable sorting
///   and equality checks during analysis.
/// - **Thread-safe and clonable:** metrics must be `Send`, `Sync`, and `Clone`.
///
/// ## Composition
/// Metrics can represent anything measurable, and can include other metrics as fields to
/// build structured, hierarchical measurements. This flexibility allows modeling of both
/// low-level (e.g., request latency) and high-level (e.g., end-to-end user transaction)
/// behaviors.
///
/// ## Example
/// ```rust
/// use karga::Metric;
/// use karga::macros::metric;
/// use std::time::Duration;
///
/// #[metric]
/// struct MyMetric {
///     latency: Duration,
///     success: bool,
///     bytes: usize,
/// }
/// ```
///
/// ## Built-in metrics
/// When the `builtins` feature is enabled, Karga provides [`BasicMetric`], a general-purpose
/// metric type containing:
/// - **latency:** duration of the operation
/// - **success:** whether the operation succeeded
/// - **bytes:** size or payload associated with the operation
///
/// This metric is sufficient for most load-testing and throughput-analysis scenarios, and is
/// the default metric type used by [`crate::aggregate::BasicAggregate`].
pub trait Metric
where
    Self: PartialOrd + PartialEq + Send + Sync + Debug + Clone,
{
}

#[cfg(feature = "builtins")]
pub use builtins::*;

#[cfg(feature = "builtins")]
mod builtins {
    use crate::macros::metric;
    use std::time::Duration;

    use super::*;

    /// A built-in [`Metric`] representing a single measurement of latency, success, and byte count.
    ///
    /// `BasicMetric` is designed to cover the most common performance-measurement needs in
    /// benchmarking and load-testing scenarios. It is minimal, efficient, and serializable.
    /// Higher-level metrics can build upon it or include additional context fields as needed.
    #[metric]
    pub struct BasicMetric {
        /// The duration of the operation.
        pub latency: Duration,
        /// Whether the operation succeeded.
        pub success: bool,
        /// The number of bytes transferred or processed.
        pub bytes: usize,
    }
}
