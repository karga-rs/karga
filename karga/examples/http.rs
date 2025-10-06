use std::time::{Duration, Instant};

use karga::{
    Report, Scenario,
    executor::ConstantExecutor,
    metrics::{BasicAggregate, BasicMetric, JsonReport},
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
        .executor(ConstantExecutor::new(Duration::from_secs(10), 12))
        .build()
        .run()
        .await
        .unwrap();

    JsonReport::from(results).report().await.unwrap();
}
