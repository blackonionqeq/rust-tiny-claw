use crate::schema::{ToolCall, ToolResult};
use crate::tools::Tool;
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct WriteFileTool {
    work_dir: PathBuf,
}

impl WriteFileTool {
    pub fn new(work_dir: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let work_dir = work_dir.into().canonicalize()?;
        Ok(Self { work_dir })
    }

    fn resolve_path_for_write(&self, path: &str) -> Result<PathBuf, String> {
        let requested = Path::new(path);
        if requested.is_absolute() {
            return Err("path must be relative to the workspace".to_string());
        }

        if requested
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(format!("path '{path}' must not contain '..'"));
        }

        let full_path = self.work_dir.join(requested);
        let Some(parent) = full_path.parent() else {
            return Err(format!("path '{path}' has no parent directory"));
        };

        // Writes are scoped to the workspace. Parent directories may not exist
        // yet, so validate the canonical parent after creating it instead of
        // canonicalizing the final file path up front.
        std::fs::create_dir_all(parent).map_err(|error| {
            format!("failed to create parent directories for '{path}': {error}")
        })?;

        let resolved_parent = parent
            .canonicalize()
            .map_err(|error| format!("failed to resolve parent directory for '{path}': {error}"))?;

        if !resolved_parent.starts_with(&self.work_dir) {
            return Err(format!("path '{path}' is outside the workspace"));
        }

        let Some(file_name) = full_path.file_name() else {
            return Err(format!("path '{path}' must name a file"));
        };

        Ok(resolved_parent.join(file_name))
    }
}

impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Create or fully overwrite a file inside the current workspace."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative file path to write, such as src/main.rs."
                },
                "content": {
                    "type": "string",
                    "description": "Complete file contents to write."
                }
            },
            "required": ["path", "content"]
        })
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        let Some(path) = call.arguments.get("path").and_then(|value| value.as_str()) else {
            return ToolResult::error(call.id.clone(), "missing string argument: path");
        };
        let Some(content) = call
            .arguments
            .get("content")
            .and_then(|value| value.as_str())
        else {
            return ToolResult::error(call.id.clone(), "missing string argument: content");
        };

        let resolved = match self.resolve_path_for_write(path) {
            Ok(path) => path,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };

        match std::fs::write(&resolved, content) {
            Ok(()) => ToolResult::ok(call.id.clone(), format!("wrote file: {path}")),
            Err(error) => ToolResult::error(
                call.id.clone(),
                format!("failed to write file '{}': {error}", resolved.display()),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WriteFileTool;
    use crate::schema::ToolCall;
    use crate::tools::Tool;
    use serde_json::json;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn write_file_creates_parent_directories_and_writes_content() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();

        let tool = WriteFileTool::new(&work_dir).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "write_file",
            json!({ "path": "src/generated.txt", "content": "hello\n" }),
        ));

        let written = fs::read_to_string(work_dir.join("src/generated.txt")).unwrap();
        fs::remove_dir_all(&work_dir).unwrap();

        assert!(!result.is_error);
        assert_eq!(written, "hello\n");
    }

    #[test]
    fn write_file_rejects_parent_directory_escape() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();

        let tool = WriteFileTool::new(&work_dir).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "write_file",
            json!({ "path": "../outside.txt", "content": "nope" }),
        ));

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("must not contain '..'"));
    }

    #[test]
    fn write_file_rejects_absolute_paths() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();
        let absolute_path = work_dir.join("file.txt");

        let tool = WriteFileTool::new(&work_dir).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "write_file",
            json!({ "path": absolute_path, "content": "nope" }),
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
