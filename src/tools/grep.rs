use crate::schema::{ToolCall, ToolResult};
use crate::tools::{Tool, ToolAccessMode};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Debug)]
pub struct GrepTool {
    work_dir: PathBuf,
    max_output_bytes: usize,
}

const DEFAULT_MAX_MATCHES: usize = 50;
const MAX_MATCHES_LIMIT: usize = 200;
const DEFAULT_CONTEXT_LINES: usize = 0;
const MAX_CONTEXT_LINES: usize = 5;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 12_000;

impl GrepTool {
    pub fn new(work_dir: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let work_dir = work_dir.into().canonicalize()?;
        Ok(Self {
            work_dir,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        })
    }

    #[cfg(test)]
    fn with_output_limit(
        work_dir: impl Into<PathBuf>,
        max_output_bytes: usize,
    ) -> Result<Self, std::io::Error> {
        let work_dir = work_dir.into().canonicalize()?;
        Ok(Self {
            work_dir,
            max_output_bytes,
        })
    }

    fn validate_optional_path(&self, path: Option<&str>) -> Result<(), String> {
        let Some(path) = path else {
            return Ok(());
        };

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

        Ok(())
    }

    fn parse_optional_usize(call: &ToolCall, name: &str) -> Result<Option<usize>, String> {
        let Some(value) = call.arguments.get(name) else {
            return Ok(None);
        };

        let Some(number) = value.as_u64() else {
            return Err(format!("argument '{name}' must be a non-negative integer"));
        };

        usize::try_from(number)
            .map(Some)
            .map_err(|_| format!("argument '{name}' is too large"))
    }

    fn parse_optional_bool(call: &ToolCall, name: &str) -> Result<Option<bool>, String> {
        let Some(value) = call.arguments.get(name) else {
            return Ok(None);
        };

        value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("argument '{name}' must be a boolean"))
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
            "{}\n\n...[grep output truncated to first {} bytes]...",
            &output[..cutoff],
            self.max_output_bytes
        )
    }

    fn rg_command(
        &self,
        query: &str,
        path: Option<&str>,
        context_lines: usize,
        max_matches: usize,
        case_sensitive: bool,
    ) -> Command {
        let mut command = Command::new("rg");
        command
            .current_dir(&self.work_dir)
            .arg("--line-number")
            .arg("--with-filename")
            .arg("--color")
            .arg("never")
            .arg("--context")
            .arg(context_lines.to_string())
            .arg("--max-count")
            .arg(max_matches.to_string());

        if !case_sensitive {
            command.arg("--ignore-case");
        }

        command.arg(query);
        if let Some(path) = path {
            command.arg(path);
        } else {
            command.arg(".");
        }

        command
    }

    fn grep_command(
        &self,
        query: &str,
        path: Option<&str>,
        context_lines: usize,
        max_matches: usize,
        case_sensitive: bool,
    ) -> Command {
        let mut command = Command::new("grep");
        command
            .current_dir(&self.work_dir)
            .arg("-R")
            .arg("-n")
            .arg("-H")
            .arg("-E")
            .arg("-I")
            .arg("--color=never")
            .arg("-C")
            .arg(context_lines.to_string())
            .arg("-m")
            .arg(max_matches.to_string());

        if !case_sensitive {
            command.arg("-i");
        }

        command.arg(query);
        if let Some(path) = path {
            command.arg(path);
        } else {
            command.arg(".");
        }

        command
    }

    fn format_command_output(
        &self,
        output: Output,
        command_name: &str,
        prefix: Option<&str>,
    ) -> String {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let body = if output.status.success() {
            if stdout.is_empty() {
                "no matches found".to_string()
            } else {
                stdout.to_string()
            }
        } else if output.status.code() == Some(1) {
            "no matches found".to_string()
        } else if stderr.is_empty() {
            format!("{command_name} exited with status {}", output.status)
        } else {
            format!(
                "{command_name} exited with status {}\n{stderr}",
                output.status
            )
        };

        let output = match prefix {
            Some(prefix) => format!("{prefix}\n{body}"),
            None => body,
        };
        self.truncate_output(output)
    }
}

impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "Search workspace files with ripgrep-compatible regular expressions."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Ripgrep-compatible regular expression to search for. Escape regex metacharacters for literal searches."
                },
                "path": {
                    "type": "string",
                    "description": "Optional workspace-relative file or directory to search, such as src or README.md."
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Whether matching should be case-sensitive. Defaults to true."
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Optional number of context lines around each match. Defaults to 0 and is capped at 5.",
                    "minimum": 0,
                    "maximum": 5
                },
                "max_matches": {
                    "type": "integer",
                    "description": "Optional maximum matches per file. Defaults to 50 and is capped at 200.",
                    "minimum": 1,
                    "maximum": 200
                }
            },
            "required": ["query"]
        })
    }

    fn access_mode(&self, _call: &ToolCall) -> ToolAccessMode {
        ToolAccessMode::ReadOnly
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        let Some(query) = call.arguments.get("query").and_then(|value| value.as_str()) else {
            return ToolResult::error(call.id.clone(), "missing string argument: query");
        };

        let path = call.arguments.get("path").and_then(|value| value.as_str());
        match self.validate_optional_path(path) {
            Ok(()) => {}
            Err(error) => return ToolResult::error(call.id.clone(), error),
        }

        let context_lines = match Self::parse_optional_usize(call, "context_lines") {
            Ok(Some(value)) => value.min(MAX_CONTEXT_LINES),
            Ok(None) => DEFAULT_CONTEXT_LINES,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };
        let max_matches = match Self::parse_optional_usize(call, "max_matches") {
            Ok(Some(0)) => {
                return ToolResult::error(
                    call.id.clone(),
                    "argument 'max_matches' must be greater than 0",
                );
            }
            Ok(Some(value)) => value.min(MAX_MATCHES_LIMIT),
            Ok(None) => DEFAULT_MAX_MATCHES,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };
        let case_sensitive = match Self::parse_optional_bool(call, "case_sensitive") {
            Ok(Some(value)) => value,
            Ok(None) => true,
            Err(error) => return ToolResult::error(call.id.clone(), error),
        };

        match self
            .rg_command(query, path, context_lines, max_matches, case_sensitive)
            .output()
        {
            Ok(output) => ToolResult::ok(
                call.id.clone(),
                self.format_command_output(output, "ripgrep", None),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => match self
                .grep_command(query, path, context_lines, max_matches, case_sensitive)
                .output()
            {
                Ok(output) => ToolResult::ok(
                    call.id.clone(),
                    self.format_command_output(
                        output,
                        "grep",
                        Some(
                            "ripgrep (rg) was not found in PATH; used grep fallback. Fallback results may not follow ripgrep ignore rules.",
                        ),
                    ),
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => ToolResult::ok(
                    call.id.clone(),
                    "neither ripgrep (rg) nor grep was found in PATH",
                ),
                Err(error) => ToolResult::error(
                    call.id.clone(),
                    format!("failed to start grep fallback: {error}"),
                ),
            },
            Err(error) => ToolResult::error(
                call.id.clone(),
                format!("failed to start ripgrep (rg): {error}"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GrepTool;
    use crate::schema::ToolCall;
    use crate::tools::{Tool, ToolAccessMode};
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn grep_searches_workspace_files() {
        let work_dir = tempdir().unwrap();
        fs::create_dir_all(work_dir.path().join("src")).unwrap();
        fs::write(
            work_dir.path().join("src/lib.rs"),
            "fn main() {}\n// TODO: auth\n",
        )
        .unwrap();

        let tool = GrepTool::new(work_dir.path()).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "grep",
            json!({ "query": "TODO", "path": "src" }),
        ));

        assert!(!result.is_error);
        if result
            .output
            .contains("neither ripgrep (rg) nor grep was found")
        {
            return;
        }
        assert!(result.output.contains("src"));
        assert!(result.output.contains("TODO: auth"));
    }

    #[test]
    fn grep_is_read_only() {
        let work_dir = tempdir().unwrap();

        let tool = GrepTool::new(work_dir.path()).unwrap();

        assert_eq!(
            tool.access_mode(&ToolCall::new("call_1", "grep", json!({ "query": "x" }))),
            ToolAccessMode::ReadOnly
        );
    }

    #[test]
    fn grep_rejects_absolute_paths() {
        let work_dir = tempdir().unwrap();
        let absolute_path = work_dir.path().join("file.txt");

        let tool = GrepTool::new(work_dir.path()).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "grep",
            json!({ "query": "x", "path": absolute_path }),
        ));

        assert!(result.is_error);
        assert!(result.output.contains("relative"));
    }

    #[test]
    fn grep_truncates_long_output() {
        let work_dir = tempdir().unwrap();
        fs::write(work_dir.path().join("a.txt"), "needle\nneedle\nneedle\n").unwrap();

        let tool = GrepTool::with_output_limit(work_dir.path(), 12).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "grep",
            json!({ "query": "needle", "path": "a.txt" }),
        ));

        assert!(!result.is_error);
        if result
            .output
            .contains("neither ripgrep (rg) nor grep was found")
        {
            return;
        }
        assert!(result.output.contains("truncated"));
    }
}
