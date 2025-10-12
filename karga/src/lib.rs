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
//! - [`Scenario`]: glue that ties everything together — defines the benchmark flow and
//!   the action(s) being measured.
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
//! # Example
//!
//! A simple HTTP example:
//!
//! ```rust
//! use std::time::{Duration, Instant};
//!
//! use karga::{
//!     Reporter, Scenario,
//!     executor::{Stage, StageExecutor},
//!     metric::{BasicAggregate, BasicMetric},
//!     report::{BasicReport, StdoutReporter},
//! };
//! use reqwest::Client;
//!
//! #[tokio::main]
//! async fn main() {
//!     // NEVER instantiate heavy objects like clients inside the action —
//!     // doing so would severely impact performance.
//!     let client = Client::new();
//!     let results = Scenario::<BasicAggregate, _, _, _>::builder()
//!         .name("HTTP scenario")
//!         .action(move || {
//!             let client = client.clone();
//!             async move {
//!                 let start = Instant::now();
//!
//!                 // For this example, we’ll hardcode the request.
//!                 let res = client.get("http://localhost:3000").send().await;
//!                 let success = match res {
//!                     Ok(r) => r.status() == 200,
//!                     Err(_) => false,
//!                 };
//!                 let elapsed = start.elapsed();
//!
//!                 BasicMetric {
//!                     latency: elapsed,
//!                     success,
//!                     // We don't use bytes in this example.
//!                     bytes: 0,
//!                 }
//!             }
//!         })
//!         .executor(
//!             StageExecutor::builder()
//!                 // We start with a certain RPS growing steadily,
//!                 // then ramp up 10× faster and return to normal.
//!                 .stages(vec![
//!                     // Using f64::MAX here means 'no rate limit'; adjust for real tests.
//!                     Stage::new(Duration::ZERO, f64::MAX),
//!                     Stage::new(Duration::from_secs(10), f64::MAX),
//!                 ])
//!                 .build(),
//!         )
//!         .build()
//!         .run()
//!         .await
//!         .unwrap();
//!
//!     let report = BasicReport::from(results);
//!     // Slightly unusual syntax, but valid.
//!     StdoutReporter {}.report(report).await.unwrap();
//! }
//! ```
//!
//! This example demonstrates how Karga combines a simple scenario, a configurable executor,
//! and built-in reporting to form a full benchmark pipeline.
//!
//! # Feature flags
//!
//! - `macros`: enables small procedural macros that reduce boilerplate when implementing
//!   common traits and registration. (Enabled by default)
//! - `builtins`: provides basic implementations (`BasicMetric`, `BasicAggregate`,
//!   `BasicReport`, `StdoutReporter`) for quick experiments and demos. (Enabled by default)
//!
//! # Where to start
//!
//! - Read the docs for [`Scenario`], [`Executor`], and [`Reporter`]. Each core trait should
//!   include an `# Examples` section that compiles and demonstrates a minimal implementation.
//! - See `examples/` for runnable scenarios (recommended: `examples/http.rs`).

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
pub use executor::{Executor, StageExecutor};
pub use metric::Metric;
pub use report::{Report, Reporter};
pub use scenario::Scenario;

#[cfg(feature = "macros")]
/// Procedural macros to reduce boilerplate
pub mod macros {
    pub use karga_macros::*;
}
