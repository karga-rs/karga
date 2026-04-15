use serde::{de::DeserializeOwned, Serialize};
use std::fmt::Debug;
use std::future::Future;

use crate::Aggregate;

/// A [`Report`] represents the processed form of an [`Aggregate`].
///
/// Reports transform raw aggregated data into meaningful insights such as
/// averages, percentiles, or ratios. They are *pure data structures*, free
/// of side effects, and encapsulate the logic needed to derive final results.
///
/// Implementors must define how to construct the report from an [`Aggregate`], typically
/// via a [`From<A>`] implementation.
///
/// # Example
///
/// ```rust
/// use karga::{Aggregate, Report, Metric};
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Clone, PartialOrd, PartialEq)]
/// struct MyMetric(u64);
/// impl Metric for MyMetric {}
///
/// #[derive(Clone)]
/// struct MyAggregate { count: u64, sum: u128 }
/// impl Aggregate for MyAggregate {
///     type Metric = MyMetric;
///     fn new() -> Self { Self { count: 0, sum: 0 } }
///     fn consume(&mut self, m: &MyMetric) { self.count += 1; self.sum += m.0 as u128; }
///     fn merge(&mut self, o: Self) { self.count += o.count; self.sum += o.sum; }
/// }
///
/// #[derive(Debug, Serialize, Deserialize)]
/// struct MyReport { average: f64 }
///
/// impl From<MyAggregate> for MyReport {
///     fn from(a: MyAggregate) -> Self {
///         Self { average: a.sum as f64 / a.count as f64 }
///     }
/// }
///
/// impl Report<MyAggregate> for MyReport {}
/// ```
pub trait Report<A>
where
    Self: Send + Sync + Debug + From<A> + Serialize + DeserializeOwned,
    A: Aggregate,
{
}

/// A [`Reporter`] consumes a [`Report`] and performs side effects such as
/// printing to stdout, sending to a database, or exporting to a monitoring system.
///
/// Reporters represent the I/O boundary of Karga. This separation allows the
/// computation layer (metrics -> aggregates -> reports) to remain pure and deterministic,
/// while reporters handle presentation and export.
///
/// # Example
///
/// ```rust
/// # use karga::{Reporter, Aggregate, Report};
/// # use std::future::Future;
/// struct StdoutReporter;
///
/// impl<A: Aggregate, R: Report<A>> Reporter<A, R> for StdoutReporter {
///     type Error = std::io::Error;
///
///     async fn report(&self, report: &R) -> Result<(), Self::Error> {
///         println!("{:?}", report);
///         Ok(())
///     }
/// }
/// ```
pub trait Reporter<A: Aggregate, R: Report<A>> {
    type Error;
    fn report(&self, report: &R) -> impl Future<Output = Result<(), Self::Error>>;
}
