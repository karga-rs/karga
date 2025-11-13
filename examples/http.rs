use std::time::{Duration, Instant};

use karga::{
    Executor, Reporter, Scenario,
    aggregate::BasicAggregate,
    executor::{Stage, StageExecutor},
    metric::BasicMetric,
    report::{BasicReport, StdoutReporter},
};
use reqwest::Client;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();
    // NEVER instantiate heavy things like clients inside the action
    // unless you want to kill performance
    let client = Client::new();

    let results: BasicAggregate = StageExecutor::builder()
        .stages(vec![
            // Start with a ramp up from 0 to 10 over 3 seconds
            Stage::new(Duration::from_secs(3), 10.0),
            // Increase the rate of change to go from 10 to 100 over the
            // next 3 seconds
            Stage::new(Duration::from_secs(3), 100.0),
            // ramp down from 100 to 10 over the next 3 sceonds
            Stage::new(Duration::from_secs(3), 10.0),
        ])
        .build()
        .exec(
            &Scenario::builder()
                .name("Http scenario")
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
                            // We dont care about it in this example
                            bytes: 0,
                        }
                    }
                })
                .build(),
        )
        .await
        .unwrap();

    let report = BasicReport::from(results);
    // Thats quite strange syntax but whatever
    StdoutReporter {}.report(&report).await.unwrap();
}
