//! Karga: The Foundational primitives for load-testing in Rust.
//!
//! Karga is a minimalist, high-performance substrate for building load-testing tools, frameworks or libraries.
//! It provides the architectural primitives required to orchestrate and measure any asynchronous workload
//! while making no assumptions about what that workload might be.
//!
//! # Core Concepts
//!
//! - **[`Metric`]**: The granular measurement produced by a single action.
//! - **[`Aggregate`]**: A mergeable collector that processes metrics into intermediate data.
//! - **[`Report`]**: A pure data transformation of aggregated data into final statistics.
//! - **[`Reporter`]**: The I/O boundary that exports reports to an external sink.
//! - **[`Scenario`]**: Ties a workload definition to an aggregation strategy.
//! - **[`Executor`]**: The runtime that drives the scenario, controlling rate and concurrency.
//!
//! # Design Goals
//!
//! - **Protocol-Agnostic**: If you can measure and fit it in an `async` function, Karga can test it.
//! - **Flexibility**: Code that is simple to provide implementations for.
//! - **Zero-Cost Abstractions**: Traits and generics are used to ensure that flexibility
//!   does not come at the cost of execution performance.

/// Metric aggregators
pub mod aggregate;
/// Orchestrators that define how things will actually run
pub mod executor;
/// Single metrics
pub mod metric;
/// Reports and Reporters
pub mod report;
/// Main module of the framework that glues everything together
pub mod scenario;

pub use aggregate::Aggregate;
pub use executor::{Executor, RateExecutor, Stage};
pub use metric::Metric;
pub use report::{Report, Reporter};
pub use scenario::Scenario;
