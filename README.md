# Karga

Foundational primitives and a high-performance execution engine for load testing in Rust.

Karga provides the foundational building blocks for load-testing: scenarios, executors, metrics, aggregates, and reporters.
It does not dictate protocol or output format; you compose the components that fit your specific requirements.

If you can write an `async` function for it, Karga can orchestrate it, scale it, and measure it. Whether you are benchmarking
HTTP APIs, gRPC streams, raw WebSockets, database driver throughput, or local file I/O. Karga makes no assumptions about your
workload.

## Core Philosophy

- **Composable**: A slim core with pluggable components.
- **Performant**: Built for low-overhead and high-precision execution.
- **Generic**: Powered by traits. If you can measure it, you can load-test it.

## Architecture

- **`Metric`**: The raw, granular measurement produced by a single execution of your workload.
- **`Aggregate`**: A fast, safely mergeable data structure that collects and compacts `Metric`s.
- **`Report`**: The final, derived statistics (e.g., percentiles, averages).
- **`Scenario`**: The definition of the workload.
- **`Executor`**: The runtime strategy that drives the scenario.
- **`Reporter`**: The I/O boundary that exports the `Report` (stdout, Prometheus, JSON, etc.).

## Usage & Documentation
Because Karga is a foundation for building tools, the best way to understand how to compose these traits is through the API documentation.
[Read the Karga documentation on docs.rs](https://docs.rs/karga) for architectural details, implementation guides, and examples.

## Built-in executors
While the workload and its metrics will vary for each use case, the execution models are much more general and do not vary as much.

Karga provides built-in executors for the most common use cases, but you are free to implement your own or use a third-party
executor that fits your system's constraints.

You can find more about the executors in the documentation.

## License

`karga` is licensed under the MIT license.
