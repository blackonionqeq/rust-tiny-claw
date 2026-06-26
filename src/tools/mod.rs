use crate::schema::{ToolCall, ToolDefinition, ToolResult};
use serde_json::json;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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

#[derive(Debug)]
pub struct BashTool {
    work_dir: PathBuf,
    timeout: Duration,
    max_output_bytes: usize,
}

impl BashTool {
    pub fn new(work_dir: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let work_dir = work_dir.into().canonicalize()?;
        Ok(Self {
            work_dir,
            timeout: Duration::from_secs(30),
            max_output_bytes: 8000,
        })
    }

    #[cfg(test)]
    fn with_limits(
        work_dir: impl Into<PathBuf>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<Self, std::io::Error> {
        let work_dir = work_dir.into().canonicalize()?;
        Ok(Self {
            work_dir,
            timeout,
            max_output_bytes,
        })
    }

    fn command_for_shell(command: &str) -> Command {
        #[cfg(windows)]
        {
            let mut shell = Command::new("bash");
            shell.arg("-lc").arg(command);
            shell
        }

        #[cfg(not(windows))]
        {
            let mut shell = Command::new("bash");
            shell.arg("-lc").arg(command);
            shell
        }
    }

    fn truncate_output(&self, output: String) -> String {
        if output.len() <= self.max_output_bytes {
            return output;
        }

        let mut cutoff = self.max_output_bytes;
        while !output.is_char_boundary(cutoff) {
            cutoff -= 1;
        }

        format!(
            "{}\n\n...[terminal output truncated to first {} bytes]...",
            &output[..cutoff],
            self.max_output_bytes
        )
    }
}

impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Run a bash command in the current workspace and return stdout and stderr."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Bash command to run in the workspace, such as cargo test or rg TODO."
                }
            },
            "required": ["command"]
        })
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        let Some(command) = call
            .arguments
            .get("command")
            .and_then(|value| value.as_str())
        else {
            return ToolResult::error(call.id.clone(), "missing string argument: command");
        };

        let mut child = match Self::command_for_shell(command)
            .current_dir(&self.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                return ToolResult::error(
                    call.id.clone(),
                    format!("failed to start bash command: {error}"),
                );
            }
        };

        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => break,
                Ok(None) if start.elapsed() < self.timeout => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Ok(None) => {
                    // Foreground commands are bounded. Long-running servers
                    // should use explicit background task tools when that
                    // runtime exists instead of relying on this timeout path.
                    let _ = child.kill();
                    let output = child.wait_with_output();
                    let output = match output {
                        Ok(output) => {
                            String::from_utf8_lossy(&output.stdout).to_string()
                                + &String::from_utf8_lossy(&output.stderr)
                        }
                        Err(error) => {
                            format!("failed to collect timed-out command output: {error}")
                        }
                    };
                    let output = self.truncate_output(output);
                    return ToolResult::ok(
                        call.id.clone(),
                        format!(
                            "{output}\n[warning: command timed out after {}s and was terminated]",
                            self.timeout.as_secs()
                        ),
                    );
                }
                Err(error) => {
                    return ToolResult::error(
                        call.id.clone(),
                        format!("failed while waiting for bash command: {error}"),
                    );
                }
            }
        }

        let output = match child.wait_with_output() {
            Ok(output) => output,
            Err(error) => {
                return ToolResult::error(
                    call.id.clone(),
                    format!("failed to collect bash command output: {error}"),
                );
            }
        };

        let output_text = String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr);
        let output_text = if output_text.is_empty() {
            "command completed successfully with no output".to_string()
        } else {
            self.truncate_output(output_text)
        };

        if output.status.success() {
            ToolResult::ok(call.id.clone(), output_text)
        } else {
            // A non-zero command exit is still an observation for the model.
            // Returning a tool error here would stop the self-correction path
            // at the engine boundary.
            ToolResult::ok(
                call.id.clone(),
                format!(
                    "command exited with status {}\n{output_text}",
                    output.status
                ),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BashTool, EchoTool, ReadFileTool, Tool, ToolRegistry, WriteFileTool};
    use crate::schema::ToolCall;
    use serde_json::json;
    use std::fs;
    use std::time::Duration;
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

    #[test]
    fn bash_runs_command_in_workspace() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();
        fs::write(work_dir.join("hello.txt"), "hello").unwrap();

        let tool = BashTool::new(&work_dir).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "bash",
            json!({ "command": "cat hello.txt" }),
        ));

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(!result.is_error);
        assert_eq!(result.output, "hello");
    }

    #[test]
    fn bash_returns_non_zero_status_as_observation() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();

        let tool = BashTool::new(&work_dir).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "bash",
            json!({ "command": "echo problem >&2; exit 7" }),
        ));

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("command exited with status"));
        assert!(result.output.contains("problem"));
    }

    #[test]
    fn bash_truncates_long_output() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();

        let tool = BashTool::with_limits(&work_dir, Duration::from_secs(30), 10).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "bash",
            json!({ "command": "printf abcdefghijklmnopqrstuvwxyz" }),
        ));

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(!result.is_error);
        assert!(result.output.starts_with("abcdefghij"));
        assert!(result.output.contains("truncated"));
    }

    fn unique_temp_dir() -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rust-tiny-claw-test-{suffix}"))
    }
}
