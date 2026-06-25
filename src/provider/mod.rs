pub trait Provider {
    fn name(&self) -> &'static str;
}

#[derive(Debug, Default)]
pub struct MockProvider;

impl Provider for MockProvider {
    fn name(&self) -> &'static str {
        "mock-provider"
    }
}
