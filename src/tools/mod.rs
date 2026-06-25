use crate::schema::{ToolCall, ToolDefinition, ToolResult};
use serde_json::json;

// Tools expose a model-facing definition and own the execution of their calls.
pub trait Tool {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> serde_json::Value;
    fn execute(&self, call: &ToolCall) -> ToolResult;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&mut self, tool: T)
    where
        T: Tool + 'static,
    {
        self.tools.push(Box::new(tool));
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|tool| tool.name()).collect()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| ToolDefinition::new(tool.name(), tool.description(), tool.input_schema()))
            .collect()
    }

    pub fn execute(&self, call: &ToolCall) -> ToolResult {
        // Unknown tools are reported as observations instead of panicking the loop.
        let Some(tool) = self.tools.iter().find(|tool| tool.name() == call.name) else {
            return ToolResult::error(
                call.id.clone(),
                format!("tool '{}' is not registered", call.name),
            );
        };

        tool.execute(call)
    }
}

#[derive(Debug, Default)]
pub struct EchoTool;

impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Placeholder tool used while the registry is being built."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to echo back as an observation."
                }
            },
            "required": ["text"]
        })
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        match call.arguments.get("text").and_then(|value| value.as_str()) {
            Some(text) => ToolResult::ok(call.id.clone(), text),
            None => ToolResult::error(call.id.clone(), "missing string argument: text"),
        }
    }
}
