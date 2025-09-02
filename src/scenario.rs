use crate::metrics::Metric;

#[derive(Debug, Clone)]
pub struct Scenario<T, F, Fut>
where
    T: Metric,
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = T> + Send,
{
    pub name: String,
    pub action: F,
}

impl<T, F, Fut> Scenario<T, F, Fut>
where
    T: Metric,
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = T> + Send,
{
    pub fn new(name: String, action: F) -> Self {
        Self { name, action }
    }
}
