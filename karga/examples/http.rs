use std::time::{Duration, Instant};

use karga::{
    Reporter, Scenario,
    aggregate::BasicAggregate,
    executor::{Stage, StageExecutor},
    metric::BasicMetric,
    report::{BasicReport, StdoutReporter},
};
use reqwest::Client;

#[tokio::main]
async fn main() {
    // NEVER intantiate heavy things like clients inside the action
    // unless you want to kill performance
    let client = Client::new();
    let results = Scenario::<BasicAggregate, _, _, _>::builder()
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
        .executor(
            StageExecutor::builder()
                // We start with a certain number of rps growing steady
                // Then we grow it 10 times faster and go back to normal
                .stages(vec![
                    Stage::new(Duration::from_secs(5), 10.0),
                    Stage::new(Duration::from_secs(5), 100.0),
                    Stage::new(Duration::from_secs(5), 10.0),
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
