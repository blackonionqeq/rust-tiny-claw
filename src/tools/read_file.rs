use crate::schema::{ToolCall, ToolResult};
use crate::tools::{Tool, ToolAccessMode};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct ReadFileTool {
    work_dir: PathBuf,
}

const DEFAULT_READ_FILE_LINE_COUNT: usize = 400;
const MAX_READ_FILE_LINE_COUNT: usize = 400;

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

    fn parse_optional_usize(call: &ToolCall, name: &str) -> Result<Option<usize>, String> {
        let Some(value) = call.arguments.get(name) else {
            return Ok(None);
        };

        let Some(number) = value.as_u64() else {
            return Err(format!("argument '{name}' must be a positive integer"));
        };

        if number == 0 {
            return Err(format!("argument '{name}' must be greater than 0"));
        }

        usize::try_from(number)
            .map(Some)
            .map_err(|_| format!("argument '{name}' is too large"))
    }

    fn format_range_output(
        path: &str,
        content: &str,
        start_line: usize,
        requested_line_count: usize,
    ) -> String {
        let lines = content.lines().collect::<Vec<_>>();
        let total_lines = lines.len();
        let line_count = requested_line_count.min(MAX_READ_FILE_LINE_COUNT);

        if total_lines == 0 {
            return format!("file: {path}\nlines: 0\n\n");
        }

        let start_index = start_line.saturating_sub(1);
        if start_index >= total_lines {
            return format!(
                "file: {path}\nlines: {start_line}-{start_line} of {total_lines}\n\n[No content: start_line is beyond the end of the file.]"
            );
        }

        let end_index = (start_index + line_count).min(total_lines);
        let displayed_start = start_index + 1;
        let displayed_end = end_index;
        let mut output =
            format!("file: {path}\nlines: {displayed_start}-{displayed_end} of {total_lines}");

        if requested_line_count > MAX_READ_FILE_LINE_COUNT {
            output.push_str(&format!(
                "\nrequested line_count {requested_line_count} was capped at {MAX_READ_FILE_LINE_COUNT}"
            ));
        }

        if end_index < total_lines {
            output.push_str(&format!(
                "\ncontent continues at line {}. Call read_file with start_line={} to continue.",
                end_index + 1,
                end_index + 1
            ));
        }

        output.push_str("\n\n");
        output.push_str(&lines[start_index..end_index].join("\n"));
        output
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
                },
                "start_line": {
                    "type": "integer",
                    "description": "Optional 1-based first line to read. Defaults to 1.",
                    "minimum": 1
                },
                "line_count": {
                    "type": "integer",
                    "description": "Optional number of lines to read. Defaults to 400 and is capped at 400.",
                    "minimum": 1,
                    "maximum": 400
                }
            },
            "required": ["path"]
        })
    }

    fn access_mode(&self, _call: &ToolCall) -> ToolAccessMode {
        ToolAccessMode::ReadOnly
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        let Some(path) = call.arguments.get("path").and_then(|value| value.as_str()) else {
            return ToolResult::error(call.id.clone(), "missing string argument: path");
        };

        let resolved = match self.resolve_path(path) {
            Ok(path) => path,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };

        let start_line = match Self::parse_optional_usize(call, "start_line") {
            Ok(Some(value)) => value,
            Ok(None) => 1,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };
        let line_count = match Self::parse_optional_usize(call, "line_count") {
            Ok(Some(value)) => value,
            Ok(None) => DEFAULT_READ_FILE_LINE_COUNT,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };

        match std::fs::read_to_string(&resolved) {
            Ok(content) => ToolResult::ok(
                call.id.clone(),
                Self::format_range_output(path, &content, start_line, line_count),
            ),
            Err(error) => ToolResult::error(
                call.id.clone(),
                format!("failed to read file '{}': {error}", resolved.display()),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReadFileTool;
    use crate::schema::ToolCall;
    use crate::tools::Tool;
    use serde_json::json;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn read_file_reads_workspace_relative_file() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(work_dir.join("src")).unwrap();
        fs::write(
            work_dir.join("src/lib.rs"),
            "pub fn answer() -> u8 { 42 }\n// done\n",
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
        assert!(result.output.contains("lines: 1-2 of 2"));
        assert!(result.output.contains("answer"));
    }

    #[test]
    fn read_file_reads_requested_line_range() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();
        fs::write(work_dir.join("long.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();

        let tool = ReadFileTool::new(&work_dir).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "read_file",
            json!({ "path": "long.txt", "start_line": 3, "line_count": 2 }),
        ));

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("lines: 3-4 of 5"));
        assert!(result.output.contains("content continues at line 5"));
        assert!(result.output.contains("three\nfour"));
        assert!(!result.output.contains("\none\n"));
    }

    #[test]
    fn read_file_caps_requested_line_count() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();
        let content = (1..=500)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(work_dir.join("long.txt"), content).unwrap();

        let tool = ReadFileTool::new(&work_dir).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "read_file",
            json!({ "path": "long.txt", "line_count": 1000 }),
        ));

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("lines: 1-400 of 500"));
        assert!(
            result
                .output
                .contains("requested line_count 1000 was capped at 400")
        );
        assert!(result.output.contains("content continues at line 401"));
    }

    #[test]
    fn read_file_rejects_invalid_range_arguments() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();
        fs::write(work_dir.join("file.txt"), "hello\n").unwrap();

        let tool = ReadFileTool::new(&work_dir).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "read_file",
            json!({ "path": "file.txt", "start_line": 0 }),
        ));

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("start_line"));
        assert!(result.output.contains("greater than 0"));
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
