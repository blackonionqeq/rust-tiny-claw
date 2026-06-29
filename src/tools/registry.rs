use crate::schema::{ToolCall, ToolDefinition, ToolResult};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

// Tools expose a model-facing definition and own the execution of their calls.
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> serde_json::Value;
    fn access_mode(&self, _call: &ToolCall) -> ToolAccessMode {
        ToolAccessMode::MutatesWorkspace
    }
    fn execute(&self, call: &ToolCall) -> ToolResult;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAccessMode {
    ReadOnly,
    MutatesWorkspace,
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<&'static str, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&mut self, tool: T) -> Result<(), ToolRegistryError>
    where
        T: Tool + 'static,
    {
        let name = tool.name();
        if self.tools.contains_key(name) {
            return Err(ToolRegistryError::DuplicateTool { name });
        }

        self.tools.insert(name, Arc::new(tool));
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut names = self.tools.keys().copied().collect::<Vec<_>>();
        names.sort_unstable();
        names
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.names()
            .into_iter()
            .filter_map(|name| self.tools.get(name))
            .map(|tool| ToolDefinition::new(tool.name(), tool.description(), tool.input_schema()))
            .collect()
    }

    pub fn execute(&self, call: &ToolCall) -> ToolResult {
        // Unknown tools are reported as observations instead of panicking the loop.
        let Some(tool) = self.tools.get(call.name.as_str()) else {
            return ToolResult::error(
                call.id.clone(),
                format!("tool '{}' is not registered", call.name),
            );
        };

        tool.execute(call)
    }

    pub fn is_read_only_call(&self, call: &ToolCall) -> bool {
        self.tools
            .get(call.name.as_str())
            .is_some_and(|tool| tool.access_mode(call) == ToolAccessMode::ReadOnly)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRegistryError {
    DuplicateTool { name: &'static str },
}

impl fmt::Display for ToolRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTool { name } => {
                write!(formatter, "tool '{name}' is already registered")
            }
        }
    }
}

impl std::error::Error for ToolRegistryError {}

#[cfg(test)]
mod tests {
    use super::{Tool, ToolRegistry};
    use crate::schema::{ToolCall, ToolResult};
    use crate::tools::ReadFileTool;
    use tempfile::tempdir;

    #[derive(Debug, Default)]
    struct TestTool;

    impl Tool for TestTool {
        fn name(&self) -> &'static str {
            "test"
        }

        fn description(&self) -> &'static str {
            "Test-only tool."
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        }

        fn execute(&self, call: &ToolCall) -> ToolResult {
            ToolResult::ok(call.id.clone(), "ok")
        }
    }

    #[test]
    fn registry_rejects_duplicate_tool_names() {
        let mut registry = ToolRegistry::new();

        registry.register(TestTool).unwrap();
        let error = registry.register(TestTool).unwrap_err();

        assert_eq!(error.to_string(), "tool 'test' is already registered");
    }

    #[test]
    fn registry_returns_definitions_in_stable_name_order() {
        let work_dir = tempdir().unwrap();

        let mut registry = ToolRegistry::new();
        registry
            .register(ReadFileTool::new(work_dir.path()).unwrap())
            .unwrap();
        registry.register(TestTool).unwrap();

        let names = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["read_file", "test"]);
    }
}
