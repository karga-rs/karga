use std::time::{Duration, Instant};

use karga::{
    Reporter, Scenario,
    executor::{Stage, StageExecutor},
    metrics::{BasicAggregate, BasicMetric},
    report::{BasicReport, StdoutReporter},
};
use reqwest::Client;

#[tokio::main]
async fn main() {
    let client = Client::new();
    let results = Scenario::<BasicAggregate, _, _, _>::builder()
        .name("Basic scenario")
        .action(move || {
            let client = client.clone();
            async move {
                let start = Instant::now();

                // Yeah lets hardcode it
                let res = client.get("http://localhost:3000").send().await;
                let success = match res {
                    Ok(r) => r.status() == 200,
                    Err(_) => false,
                };
                let elapsed = start.elapsed();
                BasicMetric {
                    latency: elapsed,
                    success,
                    bytes: 0,
                }
            }
        })
        .executor(
            StageExecutor::builder()
                .stages(vec![
                    Stage::new(Duration::ZERO, f64::MAX),
                    Stage::new(Duration::from_secs(10), f64::MAX),
                ])
                .build(),
        )
        .build()
        .run()
        .await
        .unwrap();

    let report = BasicReport::from(results);
    // Thats quite strange syntax but whatever
    StdoutReporter {}.report(report).await.unwrap();
}
