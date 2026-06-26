use crate::schema::{ToolCall, ToolResult};
use crate::tools::Tool;
use serde_json::json;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
    use super::BashTool;
    use crate::schema::ToolCall;
    use crate::tools::Tool;
    use serde_json::json;
    use std::fs;
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

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
