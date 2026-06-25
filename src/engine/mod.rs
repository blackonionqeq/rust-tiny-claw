use crate::context_engine::ContextManager;
use crate::memory::FileMemory;
use crate::provider::Provider;
use crate::telemetry::Telemetry;
use crate::tools::ToolRegistry;

pub struct AgentEngine<P> {
    provider: P,
    registry: ToolRegistry,
    context: ContextManager,
    memory: FileMemory,
    telemetry: Telemetry,
}

impl<P> AgentEngine<P>
where
    P: Provider,
{
    pub fn new(
        provider: P,
        registry: ToolRegistry,
        context: ContextManager,
        memory: FileMemory,
        telemetry: Telemetry,
    ) -> Self {
        Self {
            provider,
            registry,
            context,
            memory,
            telemetry,
        }
    }

    pub fn boot_plan(&self) -> Vec<String> {
        vec![
            format!("provider: {}", self.provider.name()),
            format!("tools registered: {}", self.registry.len()),
            format!("context manager: {}", self.context.name()),
            format!("memory root: {}", self.memory.root().display()),
            format!("telemetry: {}", self.telemetry.name()),
            "next: implement the ReAct main loop in engine/".to_string(),
        ]
    }
}
