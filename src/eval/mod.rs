//! Automated benchmark / evaluation harness.
//!
//! Mirrors the "test-driven evaluation" idea from SWE-bench: each case seeds
//! an isolated workspace, lets the engine act, then grades the result with an
//! objective assertion instead of trusting the model's self-report.
//!
//! Two tiers share one runner:
//! - [`Tier::Deterministic`] replays a scripted provider so the suite exercises
//!   real engine/tool mechanics (fuzzy edit, recovery, dispatch) with no network
//!   and no API cost. This is the CI regression gate.
//! - [`Tier::Live`] drives a real provider so the suite measures actual agent
//!   capability. It costs money and is not deterministic, so it stays opt-in.

mod scripted;

pub use scripted::ScriptedProvider;

use crate::app::build_runtime;
use crate::context_engine::ContextBudget;
use crate::engine::{AgentEngine, EngineError, RunOptions};
use crate::memory::Session;
use crate::provider::{ClaudeCompatibleProvider, OpenAiCompatibleProvider, Provider};
use crate::reporter::{Reporter, ReporterError};
use crate::schema::{Message, ToolCall, ToolResult};
use crate::telemetry::{Telemetry, TelemetryProvider};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Tier {
    Deterministic,
    Live,
}

/// How a case prepares its isolated workspace before the agent runs.
#[derive(Debug, Clone)]
pub enum Setup {
    /// Cross-platform: write `content` to workspace-relative `path`. Parent
    /// directories are created as needed. Used by the deterministic tier so it
    /// never depends on a shell.
    WriteFile { path: String, content: String },
    /// Run a POSIX shell script in the workspace. Requires `sh` on PATH
    /// (WSL / CI Linux). Used by the live tier for richer testbed setup.
    Shell(String),
}

/// How a case grades the agent's side effects after the run.
#[derive(Debug, Clone)]
pub enum Validate {
    /// Cross-platform: assert `needle` occurs in workspace-relative `path`.
    FileContains { path: String, needle: String },
    /// Run a POSIX shell script; exit 0 means pass. Same shell caveat as
    /// [`Setup::Shell`].
    Shell(String),
}

/// One independent benchmark task.
#[derive(Debug, Clone)]
pub struct TestCase {
    pub id: String,
    pub name: String,
    pub setup: Option<Setup>,
    pub prompt: String,
    pub validate: Validate,
    pub max_turns: usize,
    /// Canned model turns for the deterministic tier. Each turn with tool calls
    /// drives one ReAct step; a final tool-free turn ends the run. Ignored by
    /// the live tier.
    pub script: Option<Vec<Message>>,
}

/// Per-case outcome. `turns_used` and `tool_failures` are the two "smoothness"
/// metrics: together they separate a clean one-shot solve from a task that
/// needed many steps or recovered from tool errors.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TestResult {
    pub id: String,
    pub name: String,
    pub passed: bool,
    pub turns_used: usize,
    pub tool_failures: u64,
    pub llm_calls: u64,
    pub total_tokens: u64,
    pub elapsed_ms: u128,
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SuiteReport {
    pub tier: Tier,
    pub model_label: String,
    pub results: Vec<TestResult>,
}

pub struct BenchmarkRunner {
    tier: Tier,
    model_label: String,
    keep_workspaces: bool,
}

impl BenchmarkRunner {
    pub fn new(tier: Tier, model_label: impl Into<String>) -> Self {
        Self {
            tier,
            model_label: model_label.into(),
            keep_workspaces: false,
        }
    }

    /// Keep the per-case workspace directories on disk after the run instead of
    /// removing them, so failed agent behavior can be inspected post-mortem.
    pub fn keep_workspaces(mut self) -> Self {
        self.keep_workspaces = true;
        self
    }

    /// Run every case sequentially and return the aggregate report. Progress
    /// goes to stderr so stdout stays clean for `--json > baseline.json`; the
    /// caller picks the final stdout rendering (`print_table`, `print_summary`,
    /// or `to_json`).
    pub fn run(&self, testcases: Vec<TestCase>) -> SuiteReport {
        eprintln!("\n==================================================");
        eprintln!(
            "🚀 tiny-claw benchmark | tier: {} | model: {}",
            tier_label(self.tier),
            self.model_label
        );
        eprintln!("==================================================");

        let mut results = Vec::with_capacity(testcases.len());
        for case in &testcases {
            eprint!(">>> ⏳ [{}] {} ... ", case.id, case.name);
            let result = self.run_single(case);
            eprintln!("{}", result.one_line());
            results.push(result);
        }

        SuiteReport {
            tier: self.tier,
            model_label: self.model_label.clone(),
            results,
        }
    }

    fn run_single(&self, case: &TestCase) -> TestResult {
        let telemetry = Telemetry::default();
        let start = Instant::now();

        let work_dir = match make_work_dir(&case.id) {
            Ok(path) => path,
            Err(error) => return failed(case, error),
        };

        if let Some(setup) = &case.setup
            && let Err(error) = apply_setup(setup, &work_dir)
        {
            self.cleanup(&work_dir);
            return failed(case, error);
        }

        let (registry, context, memory) =
            match build_runtime(&work_dir, Vec::new(), telemetry.clone()) {
                Ok(parts) => parts,
                Err(error) => {
                    self.cleanup(&work_dir);
                    return failed(case, format!("engine build failed: {error}"));
                }
            };

        let provider = match self.build_provider(case, telemetry.clone()) {
            Ok(provider) => provider,
            Err(error) => {
                self.cleanup(&work_dir);
                return failed(case, error);
            }
        };

        let mut engine = AgentEngine::new(provider, registry, context, memory, telemetry.clone());
        let session = Session::new(case.id.clone(), work_dir.clone());
        let mut reporter = BenchReporter::default();
        let options = RunOptions {
            max_turns: case.max_turns,
            // Streaming hardcodes StdoutStreamSink inside the engine, which would
            // spam benchmark output. Thinking off keeps one scripted turn per
            // ReAct step. Both keep the run quiet and deterministic in shape.
            enable_thinking: false,
            plan_mode: false,
            stream: false,
            context_budget: ContextBudget::default(),
        };

        let outcome =
            engine.run_session_with_reporter(&session, &case.prompt, options, &mut reporter);
        let snapshot = telemetry.snapshot();
        let elapsed_ms = start.elapsed().as_millis();

        let (passed, error) = match outcome {
            Ok(_transcript) => run_validate(&case.validate, &work_dir),
            Err(EngineError::TurnLimitExceeded { max_turns }) => {
                (false, Some(format!("exceeded {max_turns} turn(s)")))
            }
            Err(error) => (false, Some(format!("engine error: {error}"))),
        };

        self.cleanup(&work_dir);

        TestResult {
            id: case.id.clone(),
            name: case.name.clone(),
            passed,
            turns_used: reporter.turns_used,
            tool_failures: snapshot.tools.failed_call_count,
            llm_calls: snapshot.llm.call_count,
            total_tokens: snapshot.llm.total_tokens,
            elapsed_ms,
            error,
        }
    }

    fn build_provider(
        &self,
        case: &TestCase,
        telemetry: Telemetry,
    ) -> Result<Box<dyn Provider + Send>, String> {
        let inner: Box<dyn Provider + Send> = match self.tier {
            Tier::Deterministic => Box::new(ScriptedProvider::new(
                case.script.clone().unwrap_or_default(),
            )),
            Tier::Live => Box::new(build_live_provider()?),
        };
        Ok(Box::new(TelemetryProvider::new(inner, telemetry)))
    }

    fn cleanup(&self, work_dir: &Path) {
        if !self.keep_workspaces {
            let _ = fs::remove_dir_all(work_dir);
        }
    }
}

impl SuiteReport {
    pub fn passed_count(&self) -> usize {
        self.results.iter().filter(|result| result.passed).count()
    }

    pub fn total(&self) -> usize {
        self.results.len()
    }

    pub fn pass_rate(&self) -> f64 {
        if self.results.is_empty() {
            0.0
        } else {
            self.passed_count() as f64 / self.total() as f64 * 100.0
        }
    }

    pub fn total_tokens(&self) -> u64 {
        self.results.iter().map(|result| result.total_tokens).sum()
    }

    pub fn total_tool_failures(&self) -> u64 {
        self.results.iter().map(|result| result.tool_failures).sum()
    }

    pub fn total_elapsed_ms(&self) -> u128 {
        self.results.iter().map(|result| result.elapsed_ms).sum()
    }

    pub fn print_summary(&self) {
        println!("\n================ 🏆 benchmark report ================");
        println!(
            "tier: {} | model: {}",
            tier_label(self.tier),
            self.model_label
        );
        println!(
            "cases: {} | passed: {} | pass rate: {:.2}%",
            self.total(),
            self.passed_count(),
            self.pass_rate()
        );
        println!(
            "tokens: {} | tool failures: {} | elapsed: {}ms",
            self.total_tokens(),
            self.total_tool_failures(),
            self.total_elapsed_ms()
        );
        println!("====================================================");
    }

    pub fn print_table(&self) {
        println!(
            "\n{id:<22} {name:<42} {pass:<5} {turns:<6} {fails:<6} {tokens:<8} {ms:<7}",
            id = "CASE",
            name = "NAME",
            pass = "PASS",
            turns = "TURNS",
            fails = "FAILS",
            tokens = "TOKENS",
            ms = "MS"
        );
        for result in &self.results {
            println!(
                "{id:<22} {name:<42} {pass:<5} {turns:<6} {fails:<6} {tokens:<8} {ms:<7}",
                id = result.id,
                name = truncate(&result.name, 42),
                pass = if result.passed { "yes" } else { "NO" },
                turns = result.turns_used,
                fails = result.tool_failures,
                tokens = result.total_tokens,
                ms = result.elapsed_ms
            );
        }
        self.print_summary();
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| format!("serialize failed: {error}"))
    }
}

impl TestResult {
    fn one_line(&self) -> String {
        if let Some(error) = &self.error {
            return format!("❌ {error}");
        }
        if self.passed {
            format!(
                "✅ turns={} tool_failures={} tokens={} elapsed={}ms",
                self.turns_used, self.tool_failures, self.total_tokens, self.elapsed_ms
            )
        } else {
            "❌ validation failed".to_string()
        }
    }
}

/// A silent reporter that only counts turns started, which is exactly the
/// "turns used" smoothness metric.
#[derive(Default)]
struct BenchReporter {
    turns_used: usize,
}

impl Reporter for BenchReporter {
    fn on_turn_start(&mut self, turn: usize) -> Result<(), ReporterError> {
        self.turns_used = turn;
        Ok(())
    }
    fn on_thinking_start(&mut self) -> Result<(), ReporterError> {
        Ok(())
    }
    fn on_thinking(&mut self, _content: &str) -> Result<(), ReporterError> {
        Ok(())
    }
    fn on_assistant_message(&mut self, _content: &str) -> Result<(), ReporterError> {
        Ok(())
    }
    fn on_tool_calls(&mut self, _tool_calls: &[ToolCall]) -> Result<(), ReporterError> {
        Ok(())
    }
    fn on_tool_result(&mut self, _result: &ToolResult) -> Result<(), ReporterError> {
        Ok(())
    }
    fn on_parallel_tool_batch(&mut self) -> Result<(), ReporterError> {
        Ok(())
    }
    fn on_complete(&mut self) -> Result<(), ReporterError> {
        Ok(())
    }
}

fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Deterministic => "deterministic",
        Tier::Live => "live",
    }
}

fn failed(case: &TestCase, error: String) -> TestResult {
    TestResult {
        id: case.id.clone(),
        name: case.name.clone(),
        passed: false,
        turns_used: 0,
        tool_failures: 0,
        llm_calls: 0,
        total_tokens: 0,
        elapsed_ms: 0,
        error: Some(error),
    }
}

fn make_work_dir(case_id: &str) -> Result<PathBuf, String> {
    let slug = case_id
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>();
    let sequence = WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir()
        .join("tiny-claw-bench")
        .join(format!("{slug}-{}-{sequence}", std::process::id()));
    fs::create_dir_all(&path).map_err(|error| format!("workspace create failed: {error}"))?;
    Ok(path)
}

fn apply_setup(setup: &Setup, work_dir: &Path) -> Result<(), String> {
    match setup {
        Setup::WriteFile { path, content } => {
            let target = work_dir.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("setup mkdir failed: {error}"))?;
            }
            fs::write(&target, content).map_err(|error| format!("setup write failed: {error}"))
        }
        Setup::Shell(script) => run_shell(script, work_dir),
    }
}

fn run_validate(validate: &Validate, work_dir: &Path) -> (bool, Option<String>) {
    match validate {
        Validate::FileContains { path, needle } => {
            let target = work_dir.join(path);
            match fs::read_to_string(&target) {
                Ok(content) if content.contains(needle) => (true, None),
                Ok(content) => (
                    false,
                    Some(format!(
                        "validate: '{needle}' not found in {path}; file content was: {content}"
                    )),
                ),
                Err(error) => (
                    false,
                    Some(format!("validate: cannot read {path}: {error}")),
                ),
            }
        }
        Validate::Shell(script) => match run_shell(script, work_dir) {
            Ok(()) => (true, None),
            Err(error) => (false, Some(format!("validate: {error}"))),
        },
    }
}

fn run_shell(script: &str, work_dir: &Path) -> Result<(), String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(work_dir)
        .output()
        .map_err(|error| format!("failed to spawn sh: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "shell exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn build_live_provider() -> Result<Box<dyn Provider + Send>, String> {
    let _ = dotenvy::dotenv();
    match std::env::var("TINY_CLAW_PROVIDER")
        .unwrap_or_default()
        .as_str()
    {
        "openai-compatible" => Ok(Box::new(
            OpenAiCompatibleProvider::from_env().map_err(|error| error.to_string())?,
        )),
        "claude-compatible" => Ok(Box::new(
            ClaudeCompatibleProvider::from_env().map_err(|error| error.to_string())?,
        )),
        other => Err(format!(
            "live tier requires TINY_CLAW_PROVIDER=openai-compatible|claude-compatible, got '{other}'"
        )),
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(max.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

/// A CI-runnable regression suite. The model side is fixed by scripts, so any
/// change in pass rate must come from the engine, tools, compaction, or recovery.
pub fn default_deterministic_suite() -> Vec<TestCase> {
    vec![
        TestCase {
            id: "det_edit_exact".into(),
            name: "edit_file exact-match config bump".into(),
            setup: Some(Setup::WriteFile {
                path: "config.json".into(),
                content: "{\n  \"name\": \"tiny-claw\",\n  \"version\": \"v1.0.0\"\n}\n".into(),
            }),
            prompt: "Use edit_file to change the version in config.json from v1.0.0 to v2.0.0."
                .into(),
            validate: Validate::FileContains {
                path: "config.json".into(),
                needle: "\"version\": \"v2.0.0\"".into(),
            },
            max_turns: 4,
            script: Some(vec![
                Message::assistant_with_tools(
                    "Updating the version field.",
                    vec![ToolCall::new(
                        "det_edit_exact_1",
                        "edit_file",
                        json!({
                            "path": "config.json",
                            "old_text": "\"version\": \"v1.0.0\"",
                            "new_text": "\"version\": \"v2.0.0\""
                        }),
                    )],
                ),
                Message::assistant("Done. The version is now v2.0.0."),
            ]),
        },
        TestCase {
            id: "det_edit_indent".into(),
            name: "edit_file indentation-insensitive Rust patch".into(),
            setup: Some(Setup::WriteFile {
                path: "src/main.rs".into(),
                content: "fn main() {\n    // TODO: add auth\n    if true {\n        println!(\"open\");\n    }\n}\n".into(),
            }),
            prompt: "Patch src/main.rs so unauthenticated requests are forbidden instead of open. Reuse the existing TODO line as an anchor.".into(),
            validate: Validate::FileContains {
                path: "src/main.rs".into(),
                needle: "forbidden".into(),
            },
            max_turns: 4,
            script: Some(vec![
                Message::assistant_with_tools(
                    "Replacing the open branch with a forbidden guard.",
                    vec![ToolCall::new(
                        "det_edit_indent_1",
                        "edit_file",
                        json!({
                            "path": "src/main.rs",
                            "old_text": "// TODO: add auth\nif true {\nprintln!(\"open\");\n}",
                            "new_text": "// TODO: add auth\nif user.is_none() {\n    println!(\"forbidden\");\n    return;\n}"
                        }),
                    )],
                ),
                Message::assistant("Done. Unauthenticated requests now return forbidden."),
            ]),
        },
    ]
}

/// A real-provider capability suite. Costs money and is non-deterministic; run
/// it manually with `tiny-claw-bench --live` once `TINY_CLAW_PROVIDER` and
/// `TINY_CLAW_API_KEY` are exported.
pub fn default_live_suite() -> Vec<TestCase> {
    vec![
        TestCase {
            id: "live_edit_config".into(),
            name: "edit_file config bump (real model)".into(),
            setup: Some(Setup::WriteFile {
                path: "config.json".into(),
                content: "{\n  \"name\": \"tiny-claw\",\n  \"version\": \"v1.0.0\"\n}\n".into(),
            }),
            prompt: "Change the version in config.json from v1.0.0 to v2.0.0 using edit_file. Do not touch anything else.".into(),
            validate: Validate::Shell("grep -q '\"version\": \"v2.0.0\"' config.json".into()),
            max_turns: 8,
            script: None,
        },
        TestCase {
            id: "live_write_greeting".into(),
            name: "write_file creates greeting (real model)".into(),
            setup: None,
            prompt: "Create a file named greeting.txt containing exactly the text hello-bench, using write_file.".into(),
            validate: Validate::Shell("grep -q 'hello-bench' greeting.txt".into()),
            max_turns: 8,
            script: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_contains_validate_reads_workspace_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.json"), "{\"version\": \"v2.0.0\"}").unwrap();

        let (passed, error) = run_validate(
            &Validate::FileContains {
                path: "config.json".into(),
                needle: "\"version\": \"v2.0.0\"".into(),
            },
            dir.path(),
        );

        assert!(passed);
        assert!(error.is_none());
    }

    #[test]
    fn file_contains_validate_fails_when_needle_absent() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.json"), "{\"version\": \"v1.0.0\"}").unwrap();

        let (passed, error) = run_validate(
            &Validate::FileContains {
                path: "config.json".into(),
                needle: "v2.0.0".into(),
            },
            dir.path(),
        );

        assert!(!passed);
        assert!(error.unwrap().contains("not found"));
    }

    #[test]
    fn write_file_setup_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        apply_setup(
            &Setup::WriteFile {
                path: "src/main.rs".into(),
                content: "fn main() {}".into(),
            },
            dir.path(),
        )
        .unwrap();

        let content = fs::read_to_string(dir.path().join("src/main.rs")).unwrap();
        assert_eq!(content, "fn main() {}");
    }

    #[test]
    fn pass_rate_handles_empty_suite() {
        let report = SuiteReport {
            tier: Tier::Deterministic,
            model_label: "scripted".into(),
            results: Vec::new(),
        };
        assert_eq!(report.pass_rate(), 0.0);
        assert_eq!(report.passed_count(), 0);
    }

    #[test]
    fn pass_rate_aggregates_results() {
        let report = SuiteReport {
            tier: Tier::Deterministic,
            model_label: "scripted".into(),
            results: vec![
                TestResult {
                    id: "a".into(),
                    name: "a".into(),
                    passed: true,
                    turns_used: 2,
                    tool_failures: 0,
                    llm_calls: 2,
                    total_tokens: 10,
                    elapsed_ms: 5,
                    error: None,
                },
                TestResult {
                    id: "b".into(),
                    name: "b".into(),
                    passed: false,
                    turns_used: 4,
                    tool_failures: 1,
                    llm_calls: 4,
                    total_tokens: 20,
                    elapsed_ms: 9,
                    error: Some("nope".into()),
                },
            ],
        };
        assert_eq!(report.passed_count(), 1);
        assert_eq!(report.total(), 2);
        assert_eq!(report.pass_rate(), 50.0);
        assert_eq!(report.total_tokens(), 30);
        assert_eq!(report.total_tool_failures(), 1);
    }

    #[test]
    fn report_serializes_to_json() {
        let report = SuiteReport {
            tier: Tier::Deterministic,
            model_label: "scripted".into(),
            results: vec![TestResult {
                id: "a".into(),
                name: "a".into(),
                passed: true,
                turns_used: 2,
                tool_failures: 0,
                llm_calls: 2,
                total_tokens: 10,
                elapsed_ms: 5,
                error: None,
            }],
        };
        let json = report.to_json().unwrap();
        assert!(json.contains("\"passed\": true"));
        assert!(json.contains("\"turns_used\": 2"));
    }
}
