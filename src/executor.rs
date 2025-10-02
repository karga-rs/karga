use async_trait::async_trait;

use crate::{
    metrics::{Aggregate, Reporter},
    scenario::Scenario,
};

#[async_trait]
pub trait Executor<A, R, F, Fut>
where
    Self: Send + Sync + Sized,
    A: Aggregate,
    R: Reporter,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    async fn exec(
        &self,
        scenario: &Scenario<A::Metric, R, Self, F, Fut>,
    ) -> Result<A, Box<dyn std::error::Error>>;
}
