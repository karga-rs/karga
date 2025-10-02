use typed_builder::TypedBuilder;

use crate::metrics::{Metric, Reporter};

#[derive(Debug, Clone, TypedBuilder)]
pub struct Scenario<T, R, E, F, Fut>
where
    T: Metric,
    R: Reporter,
    E: Send + Sync,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = T> + Send,
{
    pub name: String,
    pub action: F,
    pub exec: E,
    pub reporter: R,
}

impl<T, R, E, F, Fut> Scenario<T, R, E, F, Fut>
where
    T: Metric,
    R: Reporter,
    E: Send + Sync,
    F: Fn() -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = T> + Send,
{
}
