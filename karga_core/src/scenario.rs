use std::marker::PhantomData;

use typed_builder::TypedBuilder;

use crate::{executor::Executor, metrics::Aggregate};

#[derive(Debug, Clone, TypedBuilder)]
pub struct Scenario<A, E, F, Fut>
where
    A: Aggregate,
    E: Executor<A, F, Fut> + Send + Sync,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    #[builder(setter(into))]
    pub name: String,
    pub action: F,
    pub executor: E,
    #[builder(default, setter(skip))]
    aggregator: PhantomData<A>,
}

impl<A, E, F, Fut> Scenario<A, E, F, Fut>
where
    A: Aggregate,
    E: Executor<A, F, Fut> + Send + Sync,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = A::Metric> + Send,
{
    pub async fn run(&mut self) -> Result<A, Box<dyn std::error::Error>> {
        self.executor.exec(self).await
    }
}
