# karga

[![crates.io](https://img.shields.io/crates/v/karga.svg)](https://crates.io/crates/karga)

A small, flexible **load-testing core** for Rust — think *serde* but for load testing. karga provides the building blocks (scenarios, executors, metrics, aggregates, reporters) and leaves the concrete implementations to you or your ecosystem: HTTP, Kafka, filesystem or CI integration can all live in separate crates that depend on `karga`.

> **Philosophy.** karga is built for composability and extensibility. The core exposes traits and tiny primitives so consumers can implement their own executors, metrics, reporters or even whole testing frameworks on top of karga. If you like how `serde` defines traits and lets others implement `serde_json` and `serde_yaml`, you'll feel at home.

---

## Table of contents

* [Quickstart](#quickstart)
* [Core concepts](#core-concepts)
* [Example](#example)
* [Design & extensibility](#design--extensibility)
* [Roadmap](#roadmap)
* [License](#license)
* [Special Thanks](#special-thanks)

---

## Quickstart

Clone the repo and run the included example (the examples are intentionally minimal and focused on showing how to plug into karga — the `reqwest` HTTP example is just the easiest illustration):

```bash
git clone https://github.com/karga-rs/karga.git
cd karga
cargo run --example http
```

That example demonstrates measuring request latency and success (HTTP 200) using a simple executor. Replace the action with any async closure to exercise custom code (Kafka producer, filesystem workload, or any I/O you want).

---

## Core concepts

These are the primitives you will see in the codebase and should implement or compose with:

  * **Scenario** — the high-level unit of a test. It ties together a name, an action (an async closure), and an executor.
  * **Executor** — responsible for running the action (single-shot, looped, concurrent workers, etc.). Executors are intentionally externalized so you can provide different models.
  * **Metric** — a single measurement returned by an action (latency, boolean success, bytes written, etc.).
  * **Aggregate** — a way to combine many `Metric` values into summaries over time.
  * **Report** — process aggregated metrics into useful data (such as percentiles)
  * **Reporter** - sends a report somewhere: console, JSON/CSV export, HTTP endpoints, or integrations with monitoring systems.

All of these are trait-driven and designed for generics + composition.

---

## Example

Here is a full, runnable example demonstrating a staged load test against an HTTP server.

```rust
use std::time::{Duration, Instant};

use karga::{
    aggregate::BasicAggregate,
    executor::{Stage, StageExecutor},
    metric::BasicMetric,
    report::{BasicReport, StdoutReporter},
    Reporter, Scenario,
};
use reqwest::Client;

#[tokio::main]
async fn main() {
    // NEVER instantiate heavy objects like clients inside the action —
    // doing so would severely impact performance.
    let client = Client::new();
    let results: BasicAggregate = Scenario::builder()
        .name("HTTP scenario")
        .action(move || {
            let client = client.clone();
            async move {
                let start = Instant::now();

                // For this example, we’ll hardcode the request.
                let res = client.get("http://localhost:3000").send().await;
                let success = match res {
                    Ok(r) => r.status() == 200,
                    Err(_) => false,
                };
                let elapsed = start.elapsed();

                BasicMetric {
                    latency: elapsed,
                    success,
                    // We don't use bytes in this example.
                    bytes: 0,
                }
            }
        })
        .executor(
            StageExecutor::builder()
                // Define a multi-stage load profile:
                .stages(vec![
                    // 1. Ramp up from 0 to 100 RPS over 10 seconds
                    Stage::new(Duration::from_secs(10), 100.0),
                    // 2. Hold at 100 RPS for 30 seconds
                    Stage::new(Duration::from_secs(30), 100.0),
                    // 3. Ramp down to 0 RPS over 5 seconds
                    Stage::new(Duration::from_secs(5), 0.0),
                ])
                .build(),
        )
        .build()
        .run()
        .await
        .unwrap();

    let report = BasicReport::from(results);
    // Slightly unusual syntax, but valid.
    StdoutReporter {}.report(&report).await.unwrap();
}
```

The real `http` example in the repo shows a tiny `reqwest`-based action that records latency and a boolean success flag — use it as a starting point and swap the body for Kafka, gRPC or other protocols.

---

## Design & extensibility

  * **Serde-like core** — karga focuses on representing the *what* (scenarios, metrics) and not the *how*. Implementations live in separate crates.
  * **Generic-first API** — heavy use of traits and generics to make composing components ergonomic and zero-cost where possible.
  * **Closure-driven actions** — define workloads as simple async closures so users can embed arbitrary logic without boilerplate.
  * **Composable pipelines** — metrics flow from actions → aggregates → reports. Each stage is pluggable.

### How to extend

  * Implement the `Executor` trait to introduce a custom concurrency model (for example, worker pools that publish to Kafka).
  * Implement a `Reporter` to ship results to your CI, log aggregator, or a custom dashboard.
  * Write an `Metric` `Aggregate` and `Report` for some specific protocol or use case.

---

## Roadmap

Ideas and possible future additions:

  * official adapters / example crates for HTTP, gRPC, and filesystem workloads
  * CI integration helpers and example GitHub Actions workflows
  * official distributed executor


If you have a particular integration in mind (Kafka, Prometheus, a hosted service), I can help scaffold it.

---

## Contributing

Contributions welcome. Keep changes focused and idiomatic Rust. If you plan to publish a separate crate that depends on `karga`, feel free to go for it.

Please avoid using `karga` for attacks or illegal activity — this library is meant for development and testing only.

---

## License

`karga` is MIT-licensed — see the `LICENSE` file in the repository.

---

## Special Thanks

  * [k6](https://github.com/grafana/k6)
  * [goose](https://github.com/tag1consulting/goose)
  * [rlt](https://github.com/wfxr/rlt)
  * [serde](https://github.com/serde-rs/serde)

Huge thanks to those projects for ideas and inspiration.
