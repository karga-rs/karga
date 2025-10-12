//! The [`Scenario`] struct defines the orchestration layer that ties together
//! metrics, execution, and aggregation within Karga.
//!
//! A *scenario* represents a complete test or benchmark definition — it specifies
//! what to run (`action`), how to run it (`executor`), and how the results will be
//! aggregated (`Aggregate`). While other parts of Karga are meant to be swappable
//! (different metrics, executors, reporters, etc.), `Scenario` acts as the glue
//! that binds them in a cohesive workflow.
//!
//! Typically, a scenario is constructed using [`typed_builder::TypedBuilder`] and then executed via
//! [`Scenario::run`], which delegates the actual work to the provided [`Executor`].
//!
//! # Example
//! ```rust,ignore
//! use std::time::Duration;
//! use karga::Scenario;
//! use karga::metric::BasicMetric;
//! use karga::aggregate::BasicAggregate;
//!
//! // Build a scenario. The `action` produces a metric; the executor runs it.
//! let mut scenario = Scenario::builder()
//!     .name("example")
//!     .action(|| async {
//!         // Simulate work and produce a metric
//!         BasicMetric { latency: Duration::from_millis(10), success: true, bytes: 512 }
//!     })
//!     .executor(/* some executor implementing Executor<BasicAggregate, _, _> */)
//!     .build();
//!
//! // Running the scenario returns the final aggregate.
//! let result: BasicAggregate = scenario.run().await?;
//! ```
//!
//! # Design goals
//! - **Composability:** all major components (executor, aggregate, metric) remain generic.
//! - **Determinism:** scenarios define repeatable, isolated executions.
//! - **Extensibility:** advanced frameworks can provide custom scenarios, but the default
//!   implementation serves as the canonical composition model.
//!
//! # Notes on `action`
//!
//! The `action` is the user-provided function (typically an async closure) that produces a
//! single metric sample. Important guidelines:
//!
//! - **Closure capture for shared state:** the action cannot receive arguments, so capture
//!   any shared clients or resources in the closure (for example, an `reqwest::Client`).
//! - **No heavy initialization inside the action:** constructing heavy objects inside the
//!   action (such as creating a new HTTP client on every invocation) will drastically
//!   reduce throughput. In extreme cases this can change performance by orders of magnitude
//!   (e.g., a benchmark running at hundreds of thousands of RPS could collapse to only
//!   a few hundred RPS if the action creates expensive resources each call).
//! - **Prefer cloning lightweight handles:** if a client is cheaply clonable, clone it
//!   inside the closure (captured from the outer scope) and reuse the underlying connection
//!   pool or socket as appropriate.

use crate::{Aggregate, Executor};
use std::marker::PhantomData;
use typed_builder::TypedBuilder;

/// Represents a complete execution setup — action, executor, and aggregation logic.
///
/// `Scenario` is generic over four parameters:
/// - `A`: the [`Aggregate`] implementation used to accumulate metrics.
/// - `E`: the [`Executor`] responsible for managing concurrent or repeated runs.
/// - `F`: the function type that produces the asynchronous operation.
/// - `Fut`: the future returned by the action.
#[derive(Debug, Clone, TypedBuilder)]
pub struct Scenario<A, E, F, Fut>
where
    A: Aggregate,
    E: Executor<A, F, Fut> + Send + Sync,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    /// A human-readable name identifying this scenario.
    #[builder(setter(into))]
    pub name: String,

    /// The core operation producing metrics to be aggregated.
    ///
    /// This is usually an async closure returning a metric. Capture shared state
    /// outside the closure (for example, a client) and avoid heavy initialization
    /// inside the action itself to prevent severe performance degradation.
    pub action: F,

    /// The component responsible for executing the action under test.
    pub executor: E,

    /// Phantom marker connecting the scenario to its aggregate type.
    #[builder(default, setter(skip))]
    aggregator: PhantomData<A>,
}

impl<A, E, F, Fut> Scenario<A, E, F, Fut>
where
    A: Aggregate,
    E: Executor<A, F, Fut> + Send + Sync,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    /// Runs the scenario by delegating execution to the configured [`Executor`].
    ///
    /// The executor determines how the action is performed (e.g., sequentially,
    /// concurrently, or under rate control). Once complete, it returns the final
    /// aggregate summarizing all collected metrics.
    pub async fn run(&mut self) -> Result<A, Box<dyn std::error::Error>> {
        self.executor.exec(self).await
    }
}
