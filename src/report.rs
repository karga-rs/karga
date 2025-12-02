use serde::{de::DeserializeOwned, Serialize};
use std::fmt::Debug;
use std::future::Future;

use crate::Aggregate;

/// A [`Report`] represents the processed form of an [`Aggregate`].
///
/// Reports transform raw aggregated data into meaningful insights — such as
/// averages, percentiles, ratios, and totals. They are *pure data structures*, free
/// of side effects and I/O, and should encapsulate only the logic needed to derive
/// final, human- or machine-readable results.
///
/// Implementors must define how to construct the report from an [`Aggregate`], typically
/// via a [`From<A>`] implementation. Once created, a report can be serialized, logged,
/// or consumed by a [`Reporter`].
///
/// # Design goals
/// - **Purity:** reports contain no I/O; they are deterministic data transformations.
/// - **Serializability:** all reports must implement [`Serialize`] and [`DeserializeOwned`].
/// - **Composability:** the same aggregate type can feed multiple report implementations
///   with different analytical focuses.
///
/// # Example
/// ```rust, ignore
/// use karga::{Aggregate, Report};
/// use serde::{Serialize, Deserialize};
/// use std::time::Duration;
///
/// #[derive(Debug, Serialize, Deserialize)]
/// struct MyReport {
///     average_latency: Duration,
/// }
///
/// impl From<MyAggregate> for MyReport {
///     fn from(a: MyAggregate) -> Self {
///         Self { average_latency: a.total_latency / a.count as u32 }
///     }
/// }
///
/// impl Report<MyAggregate> for MyReport {}
/// ```
///
/// See also: [`Reporter`].
pub trait Report<A>
where
    Self: Send + Sync + Debug + From<A> + Serialize + DeserializeOwned,
    A: Aggregate,
{
}

/// A [`Reporter`] consumes a [`Report`] and performs side effects — displaying it,
/// sending it to a service, or persisting it somewhere.
///
/// Reporters represent the I/O boundary of Karga. They may be synchronous or async,
/// and can target multiple destinations. This separation allows the computation layer
/// (metrics → aggregates → reports) to remain pure and deterministic, while reporters
/// handle presentation and export.
///
/// # Example
/// ```rust
/// use karga::{Reporter, Aggregate, Report};
/// struct MyReporter;
/// impl<A: Aggregate, R: Report<A>> Reporter<A, R> for MyReporter {
///     async fn report(&self, report: &R) -> Result<(), Box<dyn std::error::Error>> {
///         println!("{:?}", report);
///         Ok(())
///     }
/// }
/// ```
pub trait Reporter<A: Aggregate, R: Report<A>> {
    fn report(&self, report: &R) -> impl Future<Output = Result<(), Box<dyn std::error::Error>>>;
}
