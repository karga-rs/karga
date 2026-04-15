use crate::Metric;

/// The [`Aggregate`] trait defines how raw [`Metric`] values are collected and combined
/// into a mergeable representation that preserves the information necessary for analysis.
///
/// **Important:** `Aggregate` implementations should **not** compute final statistics
/// such as averages or percentiles. Those derived values belong in a [`crate::Report`].
/// Aggregates are responsible for storing compact, mergeable raw data (counts, sums,
/// histograms, or sketches) so that the reporting stage can compute accurate summaries.
///
/// # Design Goals
///
/// - **Preserve Information:** Use representations that allow later computation of
///   percentiles and rates without needing raw per-sample retention.
/// - **Efficient Merging:** `consume` and `merge` should be optimized for frequent updates.
///   Merging must be **associative** and **commutative**.
///
/// # Example
///
/// ```rust
/// use karga::{Aggregate, Metric};
///
/// #[derive(Clone, PartialOrd, PartialEq)]
/// struct MyMetric(u64);
/// impl Metric for MyMetric {}
///
/// #[derive(Clone)]
/// struct MyAggregate {
///     count: u64,
///     sum: u128,
/// }
///
/// impl Aggregate for MyAggregate {
///     type Metric = MyMetric;
///
///     fn new() -> Self {
///         Self { count: 0, sum: 0 }
///     }
///
///     fn consume(&mut self, metric: &Self::Metric) {
///         self.count += 1;
///         self.sum += metric.0 as u128;
///     }
///
///     fn merge(&mut self, other: Self) {
///         self.count += other.count;
///         self.sum += other.sum;
///     }
/// }
/// ```
///
/// # Provided methods
/// - [`Aggregate::aggregate`]: a convenience helper that calls [`Aggregate::consume`] for each metric in a slice.
///
/// # Implementor notes
/// - Ensure `merge` is **associative** and **commutative** so that merging order does not
///   affect results when combining worker-local aggregates.
/// - Do not perform final derivations (like computing percentiles or averages) in the
///   aggregate — leave those calculations to the `Report` stage so different reporting
///   formats can derive the statistics they need from the same raw aggregate.
/// - Document the accuracy/memory trade-offs of your aggregate representation so users
///   understand how it affects final report fidelity.
///
/// # Reporter boundary
///
/// Once a `Report` derives human or machine-friendly statistics from one or more
/// `Aggregate`s, a `Reporter` is responsible for converting that `Report` into the
/// desired sink (stdout, file, database, telemetry system, etc.). Reporters are free to
/// format, compress, or enrich reports as needed.
pub trait Aggregate
where
    Self: Send + Sync + Clone,
{
    /// The metric type this aggregate summarizes.
    type Metric: Metric;

    /// Create a new, empty instance of the aggregate.
    fn new() -> Self;

    /// Aggregate multiple metrics into the current instance.
    ///
    /// This default implementation calls [`Aggregate::consume`] for each metric.
    fn aggregate(&mut self, metrics: &[Self::Metric]) {
        metrics.iter().for_each(|m| self.consume(m));
    }

    /// Incorporate a single metric into the aggregate.
    fn consume(&mut self, metric: &Self::Metric);

    /// Combine two different aggregates into one.
    fn merge(&mut self, other: Self);
}
