//! Executor — orchestration of runtime execution and rate control
//!
//! The `Executor` trait is the runtime that executes a `Scenario`. Different
//! executors provide different execution strategies: sequential, concurrent,
//! distributed, or token-bucket-based.
//!
//! Karga provides a built-in [`RateExecutor`] which uses a high-precision and
//! high-performance rate control system to manage throughput across multiple
//! [`Stage`]s.

#[cfg(feature = "tokio")]
pub mod rate;
use crate::{aggregate::Aggregate, scenario::Scenario};
#[cfg(feature = "tokio")]
pub use rate::{RateExecutor, Stage};

use std::future::Future;

/// The [`Executor`] is the runtime that drives a [`Scenario`].
///
/// It defines the execution strategy, such as:
/// - Controlling throughput (e.g., token-bucket or RPS-based).
/// - Managing concurrency and worker spawning.
/// - Collecting and merging results from the scenario action.
///
/// Karga provides built-in executors like [`RateExecutor`], but the trait is
/// generic to allow for custom scheduling or distributed execution strategies.
pub trait Executor<A, F, Fut>
where
    Self: Send + Sync + Sized,
    A: Aggregate,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    type Error;

    /// Execute the scenario and return the final aggregate.
    ///
    /// This function orchestrates the spawning of workers, management of
    /// rate limits, and the final collection of [`Aggregate`] data.
    fn exec(
        &self,
        scenario: &Scenario<A, F, Fut>,
    ) -> impl Future<Output = Result<A, Self::Error>> + Send;
}
