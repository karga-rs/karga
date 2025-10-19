//! Executor — orchestration of runtime execution and rate control
//!
//! The `Executor` trait is the runtime hook that executes a `Scenario`. Different
//! executors provide different execution strategies: sequential, concurrent,
//! distributed, or token-bucket-based.
//!
//! The `StageExecutor` below implements a token-bucket governor driven by a list of
//! `Stage`s. Each `Stage` defines a target requests-per-second (RPS) and a duration
//! over which the governor will smoothly interpolate from the previous rate to the
//! stage's target. The governor periodically adds tokens to a shared atomic counter
//! and worker tasks claim tokens (one token per request) using CAS.
//!
//! This design separates **rate generation** (governor) from **work execution**
//! (workers) and keeps the hot-path in workers focused on calling the user's `action`.
//!
//! # High-level flow
//! 1. Spawn a governor task that increments the shared `tokens` counter according to
//!    the current, interpolated rate.
//! 2. Spawn N worker tasks. Each worker repeatedly:
//!    - waits until the benchmark `start` flag is set,
//!    - tries to claim a token (CAS on `tokens`), sleeping briefly if none are available,
//!    - when a token is claimed, calls the `action()` future to produce a `Metric`, and
//!      consumes it into a worker-local `Aggregate`.
//! 3. When the governor finishes all stages it exits; the executor sets `shutdown` and
//!    collects aggregates from all workers, merging them to produce the final result.
//!
//! # Tuning knobs
//! - `tick` (Duration): granularity of governor updates. Smaller ticks reduce
//!   quantization error but cause more wakeups and overhead. Typical values: 10–200ms.
//! - `bucket_capacity` (u64): maximum stored tokens for absorbing bursts. A small
//!   capacity limits bursts; a larger capacity allows short spikes.
//! - `workers` (usize): number of worker tasks. Each worker is an async task that
//!   awaits tokens and runs the provided `action`. Default is `num_cpus * 120`.
//!
//! # Notes about correctness & robustness
//! - `merge` on aggregates must be associative & commutative; worker-local aggregates
//!   are merged in arbitrary order.
//! - Panics inside a worker task will cause that task to abort; the current executor
//!   uses `expect` on worker joins which will propagate panics as program panics.
//! - The governor uses floating-point interpolation to compute per-tick additions.
//!   To avoid starving fractional contributions, a small `fractional` accumulator is
//!   carried across ticks and converted to integer tokens when it accumulates enough
//!   fractional parts.
//!
//! # Mathematical behavior of the governor
//! For a given stage with `start_rate` (previous rate) and `end_rate` (stage.target)
//! over `duration`, at time `elapsed` the instantaneous rate `r(t)` is computed by
//! linear interpolation:
//!
//! ```text
//! t = elapsed / duration
//! r(t) = start_rate + (end_rate - start_rate) * t
//! ```
//!
//! The governor then computes how many tokens to add in a tick of length `tick`:
//!
//! ```text
//! add_f = r(t) * tick_seconds
//! add_total = floor(add_f + fractional)
//! fractional = (add_f + fractional) - add_total
//! ```
//!
//! `add_total` tokens are added atomically (saturating at `bucket_capacity`). This
//! spreads the continuous rate into discrete request tokens while preserving the
//! long-term average.
//!
//! # Deep dive & implementation notes
//!
//! ## Fractional carrying
//! The `fractional` accumulator ensures that small fractional contributions (e.g. 0.3
//! tokens per tick) are not lost. By accumulating these fractions across ticks, the
//! governor preserves the expected total number of tokens over time and avoids bias.
//!
//! ## Why CAS loops?
//! The token counter is an `AtomicU64`. To avoid races when multiple governor ticks
//! or workers update it concurrently we use a CAS (`compare_exchange`) loop. This
//! ensures correctness without a global lock and provides good performance under
//! contention.
//!
//! ## Worker token-claim strategy
//! Workers perform a fast `load` and, if tokens are available, attempt a `compare_exchange`
//! to claim one token. If the `compare_exchange` fails (another worker claimed the token),
//! the worker retries. If no tokens are available, workers sleep briefly (1ms) to avoid
//! busy-waiting. This sleep value is a trade-off: smaller sleeps reduce latency but
//! increase CPU usage; larger sleeps reduce CPU but increase token claim latency.
//!
//! ## Start & shutdown coordination
//! `start` and `shutdown` are `AtomicBool`s used to coordinate lifecycle. Workers spin
//! until `start` is true, then begin normal operation. The governor finishing its
//! stages signals that the run is complete; the executor sets `shutdown` and waits for
//! workers to drain.
//!
//! # Common pitfalls & recommendations
//! - **Do not perform blocking I/O inside the `action`.** Use async clients and non-blocking
//!   primitives. Blocking inside the action will stall workers and distort the RPS.
//! - **Avoid heavy allocations per action invocation.** Allocate buffers outside the action
//!   when possible and reuse them.
//! - **Tune `tick` for your workload.** Very small ticks (e.g., <10ms) increase scheduler
//!   churn. Very large ticks (e.g., >1s) cause coarse-grained rate control and visible
//!   sawtooth effects in throughput.
//! - **Choose workers count carefully.** Too few workers limit concurrency; too many waste
//!   memory and scheduler time. The `num_cpus * 120` default is empirically tuned for
//!   high-throughput async workloads but might be excessive for CPU-bound actions.
pub mod stage;
pub use stage::{Stage, StageExecutor};

use crate::{aggregate::Aggregate, scenario::Scenario};

pub trait Executor<A, F, Fut>
where
    Self: Send + Sync + Sized,
    A: Aggregate,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    /// Execute the scenario and return the final aggregate.
    fn exec(
        &self,
        scenario: &Scenario<A, Self, F, Fut>,
    ) -> impl Future<Output = Result<A, Box<dyn std::error::Error>>> + Send;
}
