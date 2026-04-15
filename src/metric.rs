/// A [`Metric`] represents a single observed measurement produced by a workload action.
///
/// Metrics are the most granular level of performance or behavioral data. They capture
/// the raw results of an operation—such as latency, success/failure, or throughput—which
/// are later collected and summarized by an [`crate::Aggregate`].
///
/// ## Requirements
/// - **Comparable:** Metrics must support [`PartialEq`] and [`PartialOrd`] to allow sorting
///   and analysis.
/// - **Thread-safe:** Metrics must be `Send + Sync + Clone` to be moved between worker tasks
///   and aggregation buffers.
///
/// ## Example
///
/// ```rust
/// use karga::Metric;
/// use std::time::Duration;
///
/// #[derive(Clone, PartialOrd, PartialEq)]
/// struct MyMetric {
///     latency: Duration,
///     success: bool,
/// }
///
/// impl Metric for MyMetric {}
/// ```
pub trait Metric
where
    Self: PartialOrd + PartialEq + Send + Sync + Clone,
{
}
