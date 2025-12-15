//! Executor — orchestration of runtime execution and rate control
//!
//! The `Executor` trait is the runtime that executes a `Scenario`. Different
//! executors provide different execution strategies: sequential, concurrent,
//! distributed, or token-bucket-based.
//!
//! Karga provides a built-in [`StageExecutor`] which uses a token-bucket governor
//! driven by a list of [`Stage`]s to control the request rate.
pub mod stage;
pub use stage::{Stage, StageExecutor};

use crate::{aggregate::Aggregate, scenario::Scenario};
use std::future::Future;

/// The runtime hook that executes a `Scenario`.
///
/// `Executor` defines the execution strategy for a given scenario, such as:
/// - Simple sequential or concurrent runs.
/// - Rate-limited execution (e.g., token-bucket).
/// - Distributed execution across multiple nodes.
///
/// This trait is generic over the aggregate, action, and future types to remain
/// flexible and composable.
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
    /// This function is responsible for implementing the execution strategy,
    /// such as spawning workers, managing concurrency, and collecting
    /// results from the `scenario.action`.
    fn exec(
        &self,
        scenario: &Scenario<A, F, Fut>,
    ) -> impl Future<Output = Result<A, Self::Error>> + Send;
}
