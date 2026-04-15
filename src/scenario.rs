//! The [`Scenario`] ties an async action to an aggregation strategy.
//!
//! A scenario is a configuration object that defines *what* to run (the `action`)
//! and *how* to collect the results ([`Aggregate`]). It is protocol-agnostic; the
//! action can be any async function returning a [`Metric`].
//!
//! # Example
//!
//! ```rust
//! use std::time::Duration;
//! use karga::{Scenario, Metric, Aggregate};
//!
//! #[derive(Clone, PartialEq, PartialOrd)]
//! struct MyMetric(Duration);
//! impl Metric for MyMetric {}
//!
//! #[derive(Clone)]
//! struct MyAggregate(u64);
//! impl Aggregate for MyAggregate {
//!     type Metric = MyMetric;
//!     fn new() -> Self { Self(0) }
//!     fn consume(&mut self, _: &Self::Metric) { self.0 += 1; }
//!     fn merge(&mut self, other: Self) { self.0 += other.0; }
//! }
//!
//! let scenario: Scenario<MyAggregate, _, _> = Scenario::builder()
//!     .name("my-workload")
//!     .action(|| async {
//!         // Perform work...
//!         MyMetric(Duration::from_millis(10))
//!     })
//!     .build();
//! ```
//!
//! # Performance Guidelines
//!
//! The `action` is executed repeatedly by the executor. To maintain high throughput:
//! - **Capture state:** Capture shared resources (like HTTP clients) in the closure.
//! - **Avoid heavy init:** Do not create expensive resources (clients, pools) inside the action.
//! - **Cheap clones:** Clone lightweight handles (e.g., `Arc<T>`) inside the closure if needed.

use crate::Aggregate;
use std::future::Future;
use std::marker::PhantomData;
use typed_builder::TypedBuilder;

/// Represents a complete execution setup — action, executor, and aggregation logic.
///
/// `Scenario` is generic over four parameters:
/// - `A`: the [`Aggregate`] implementation used to accumulate metrics.
/// - `F`: the function type that produces the asynchronous operation.
/// - `Fut`: the future returned by the action.
#[derive(Debug, Clone, TypedBuilder)]
pub struct Scenario<A, F, Fut>
where
    A: Aggregate,
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

    /// Phantom marker connecting the scenario to its aggregate type.
    #[builder(default, setter(skip))]
    aggregator: PhantomData<A>,
}
