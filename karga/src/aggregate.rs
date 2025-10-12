use crate::Metric;
use serde::{Serialize, de::DeserializeOwned};
use std::fmt::Debug;

/// The `Aggregate` trait defines how raw [`Metric`] values are collected and combined
/// into an intermediate, mergeable representation that preserves the information
/// necessary for later analysis.
///
/// **Important:** `Aggregate` implementations should **not** compute final statistics
/// such as averages or percentiles. Those derived values belong in a [`Report`], which
/// is converted from an `Aggregate` and performs the final processing. Aggregates are
/// responsible for storing compact, mergeable raw data (counts, sums, histograms,
/// sketches, error counters, etc.) so that the `Report` stage can compute accurate
/// summaries without losing information.
///
/// # Role
///
/// - Collect individual [`Metric`] samples produced by a `Scenario` action.
/// - Store the minimal but sufficient information needed to compute final statistics
///   later (for example: per-bucket histograms, counters, and totals).
/// - Be cheaply mergeable so multiple worker-local aggregates can be combined into a
///   global view.
///
/// # Design goals
///
/// - **Preserve information:** prefer representations that allow later computation of
///   percentiles, rates, and error ratios without needing raw per-sample retention.
/// - **Serializable:** implement [`Serialize`] and [`DeserializeOwned`] to permit
///   persistence or transferring aggregates across threads or processes.
/// - **Efficient to update & merge:** `consume` and `merge` should be optimized for
///   frequent updates and parallel merges.
///
/// # Memory vs accuracy
///
/// Aggregates often embody a trade-off between memory use and analytic fidelity. A
/// histogram with many buckets gives more accurate percentiles but consumes more memory;
/// a compact sketch (e.g., t-digest) reduces memory at the cost of some precision. Pick
/// the representation appropriate for your workload and document its error characteristics
/// in the corresponding `Report` implementation.
///
/// # Example
/// ```rust
/// use karga::{Aggregate, Metric, macros::*};
///
/// #[metric]
/// struct MyMetric(u64);
///
/// #[aggregate]
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
/// - [`aggregate`]: a convenience helper that calls [`consume`] for each metric in a slice.
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
/// Once a `Report` derives human- or machine-friendly statistics from one or more
/// `Aggregate`s, a `Reporter` is responsible for converting that `Report` into the
/// desired sink (stdout, file, database, telemetry system, etc.). Reporters are free to
/// format, compress, or enrich reports as needed.
pub trait Aggregate
where
    Self: Serialize + DeserializeOwned + PartialOrd + PartialEq + Send + Sync + Debug + Clone,
{
    /// The metric type this aggregate summarizes.
    type Metric: Metric;

    /// Create a new, empty instance of the aggregate.
    fn new() -> Self;

    /// Aggregate multiple metrics into the current instance.
    ///
    /// This default implementation calls [`consume`] for each metric.
    fn aggregate(&mut self, metrics: &[Self::Metric]) {
        metrics.iter().for_each(|m| self.consume(m));
    }

    /// Incorporate a single metric into the aggregate.
    fn consume(&mut self, metric: &Self::Metric);

    /// Combine two different aggregates into one.
    fn merge(&mut self, other: Self);
}

#[cfg(feature = "builtins")]
pub use builtins::*;

#[cfg(feature = "builtins")]
mod builtins {
    use std::time::Duration;

    use crate::metric::BasicMetric;

    use super::*;
    use crate::macros::aggregate;

    /// The default built-in implementation of [`Aggregate`].
    ///
    /// `BasicAggregate` provides a minimal, general-purpose accumulator that tracks:
    /// - **Total latency:** the sum of all request latencies.
    /// - **Success count:** how many requests succeeded.
    /// - **Total bytes:** the total number of bytes transferred.
    /// - **Count:** the total number of samples recorded.
    ///
    /// These are intentionally low-level metrics — a [`Report`] can later derive averages,
    /// percentiles, and success ratios from this raw data without losing precision.
    ///
    /// Enabled via the `builtins` feature.
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
