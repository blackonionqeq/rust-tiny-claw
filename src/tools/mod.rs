use crate::schema::{ToolCall, ToolDefinition, ToolResult};
use serde_json::json;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

// Tools expose a model-facing definition and own the execution of their calls.
pub trait Tool {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> serde_json::Value;
    fn execute(&self, call: &ToolCall) -> ToolResult;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<&'static str, Box<dyn Tool>>,
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

        self.tools.insert(name, Box::new(tool));
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

#[derive(Debug)]
pub struct ReadFileTool {
    work_dir: PathBuf,
}

impl ReadFileTool {
    pub fn new(work_dir: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let work_dir = work_dir.into().canonicalize()?;
        Ok(Self { work_dir })
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf, String> {
        let requested = Path::new(path);
        if requested.is_absolute() {
            return Err("path must be relative to the workspace".to_string());
        }

        let full_path = self.work_dir.join(requested);
        let resolved = full_path
            .canonicalize()
            .map_err(|error| format!("failed to resolve path '{path}': {error}"))?;

        if !resolved.starts_with(&self.work_dir) {
            return Err(format!("path '{path}' is outside the workspace"));
        }

        Ok(resolved)
    }
}

impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a file inside the current workspace."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative path to read, such as Cargo.toml or src/main.rs."
                }
            },
            "required": ["path"]
        })
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        let Some(path) = call.arguments.get("path").and_then(|value| value.as_str()) else {
            return ToolResult::error(call.id.clone(), "missing string argument: path");
        };

        let resolved = match self.resolve_path(path) {
            Ok(path) => path,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };

        match std::fs::read_to_string(&resolved) {
            Ok(content) => ToolResult::ok(call.id.clone(), content),
            Err(error) => ToolResult::error(
                call.id.clone(),
                format!("failed to read file '{}': {error}", resolved.display()),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EchoTool, ReadFileTool, Tool, ToolRegistry};
    use crate::schema::ToolCall;
    use serde_json::json;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn registry_rejects_duplicate_tool_names() {
        let mut registry = ToolRegistry::new();

        registry.register(EchoTool).unwrap();
        let error = registry.register(EchoTool).unwrap_err();

        assert_eq!(error.to_string(), "tool 'echo' is already registered");
    }

    #[test]
    fn registry_returns_definitions_in_stable_name_order() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();

        let mut registry = ToolRegistry::new();
        registry
            .register(ReadFileTool::new(&work_dir).unwrap())
            .unwrap();
        registry.register(EchoTool).unwrap();

        let names = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();

        fs::remove_dir_all(&work_dir).unwrap();

        assert_eq!(names, vec!["echo", "read_file"]);
    }

    #[test]
    fn read_file_reads_workspace_relative_file() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(work_dir.join("src")).unwrap();
        fs::write(
            work_dir.join("src/lib.rs"),
            "pub fn answer() -> u8 { 42 }\n",
        )
        .unwrap();

        let tool = ReadFileTool::new(&work_dir).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "read_file",
            json!({ "path": "src/lib.rs" }),
        ));

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("answer"));
    }

    #[test]
    fn read_file_rejects_absolute_paths() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();
        let absolute_path = work_dir.join("Cargo.toml");

        let tool = ReadFileTool::new(&work_dir).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "read_file",
            json!({ "path": absolute_path }),
        ));

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("relative"));
    }

    fn unique_temp_dir() -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rust-tiny-claw-test-{suffix}"))
    }
}
