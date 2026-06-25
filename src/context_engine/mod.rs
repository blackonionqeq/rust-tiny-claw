#[derive(Debug, Default)]
pub struct ContextManager;

impl ContextManager {
    pub fn name(&self) -> &'static str {
        "context-manager"
    }
}
