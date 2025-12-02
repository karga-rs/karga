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
/// use std::time::Duration;
///
/// #[derive(Clone, PartialOrd, PartialEq)]
/// struct MyMetric {
///     latency: Duration,
///     success: bool,
///     bytes: usize,
/// }
/// impl Metric for MyMetric{}
/// ```
pub trait Metric
where
    Self: PartialOrd + PartialEq + Send + Sync + Clone,
{
}
