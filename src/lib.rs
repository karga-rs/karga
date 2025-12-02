//! Karga — a small, flexible load-testing framework for Rust.
//!
//! Karga is inspired by the design of Serde (a slim core with pluggable extensions)
//! and by tools such as K6, RLT, and Goose for practical load-testing concerns.
//!
//! The library is intentionally minimal: you provide small building blocks (metrics,
//! aggregates, reporters, executors) and compose them into a `Scenario`. For convenience,
//! there are a few built-in implementations that cover common use cases.
//!
//! # Architecture
//!
//! The main building blocks are:
//!
//! - [`Scenario`]: configuration object that defines the action to be executed
//! - [`Executor`]: responsible for actually running the scenario. Executors control
//!   concurrency, scheduling, and are the primary place where performance matters. We provide
//!   a high-performance `StageExecutor`, but executors are replaceable.
//! - [`Metric`]: the smallest unit produced by an action. A scenario’s action returns a
//!   `Metric` describing a single sample.
//! - [`Aggregate`]: a lightweight, specialized collector that knows how to process
//!   `Metric`s into a compact intermediate representation.
//! - [`Report`]: transforms an `Aggregate` into human- or machine-friendly output.
//! - [`Reporter`]: consumes `Report`s and sends them somewhere (stdout, file, database).
//!
//! # Design goals
//!
//! - Small, well-documented core that is easy to extend.
//! - High performance in the executor layer — low allocation overhead and efficient
//!   scheduling are the primary optimizations.
//! - Composability: users can supply their own metrics/aggregates/reporters or use the
//!   built-ins for convenience.
//!
//!
//! # Feature flags
//! - `internals`: enable access to internal (and unstable) functions and useful implementation resources
//!
//! # Where to start
//!
//! - Read the docs for [`Scenario`], [`Executor`], and [`Reporter`]. Each core trait should
//!   include an `# Examples` section that compiles and demonstrates a minimal implementation.

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
pub use executor::{Executor, Stage, StageExecutor};
pub use metric::Metric;
pub use report::{Report, Reporter};
pub use scenario::Scenario;
