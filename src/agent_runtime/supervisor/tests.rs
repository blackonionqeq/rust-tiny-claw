use super::*;
use crate::provider::Provider;
use crate::schema::ToolDefinition;
use crate::tools::{GrepTool, LoadSkillTool, ReadFileTool, Tool, ToolAccessMode};
use serde_json::json;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use tempfile::TempDir;
use tempfile::tempdir;

#[derive(Clone, Default)]
struct ProviderTrace {
    calls: Arc<Mutex<Vec<ProviderCall>>>,
}

#[derive(Clone, Debug)]
struct ProviderCall {
    messages: Vec<Message>,
    tool_names: Vec<String>,
}

struct ScriptedProviderFactory {
    outputs: Arc<Mutex<Vec<Result<Message, ProviderError>>>>,
    trace: ProviderTrace,
}

impl ScriptedProviderFactory {
    fn new(outputs: Vec<Result<Message, ProviderError>>) -> Self {
        Self {
            outputs: Arc::new(Mutex::new(outputs)),
            trace: ProviderTrace::default(),
        }
    }

    fn trace(&self) -> ProviderTrace {
        self.trace.clone()
    }
}

impl ProviderFactory for ScriptedProviderFactory {
    fn create(&self) -> Result<Box<dyn Provider + Send>, ProviderError> {
        Ok(Box::new(ScriptedProvider {
            outputs: Arc::clone(&self.outputs),
            trace: self.trace.clone(),
        }))
    }
}

struct FailingProviderFactory;

impl ProviderFactory for FailingProviderFactory {
    fn create(&self) -> Result<Box<dyn Provider + Send>, ProviderError> {
        Err(ProviderError::new("provider unavailable"))
    }
}

struct ScriptedProvider {
    outputs: Arc<Mutex<Vec<Result<Message, ProviderError>>>>,
    trace: ProviderTrace,
}

impl Provider for ScriptedProvider {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn generate(
        &mut self,
        messages: &[Message],
        available_tools: Option<&[ToolDefinition]>,
    ) -> Result<Message, ProviderError> {
        self.trace.calls.lock().unwrap().push(ProviderCall {
            messages: messages.to_vec(),
            tool_names: available_tools
                .unwrap_or_default()
                .iter()
                .map(|tool| tool.name.clone())
                .collect(),
        });

        self.outputs.lock().unwrap().remove(0)
    }
}

struct MutatingTestTool;

impl Tool for MutatingTestTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Test-only mutating tool."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {} })
    }

    fn access_mode(&self, _call: &ToolCall) -> ToolAccessMode {
        ToolAccessMode::MutatesWorkspace
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        ToolResult::error(
            call.id.clone(),
            "write_file should not be callable by subagents",
        )
    }
}

#[test]
fn delegate_join_runs_tool_loop_returns_report_and_persists_files() {
    let fixture = SupervisorFixture::new();
    let factory = Arc::new(ScriptedProviderFactory::new(vec![
        Ok(Message::assistant_with_tools(
            "I will inspect the target file.",
            vec![ToolCall::new(
                "call_read",
                "read_file",
                json!({ "path": "src/lib.rs" }),
            )],
        )),
        Ok(Message::assistant(
            "## Summary\n\nFound the answer function.\n\n## Evidence\n\n- `src/lib.rs` defines `answer`.\n\n## Uncertainty\n\nNone",
        )),
    ]));
    let trace = factory.trace();
    let supervisor = fixture.supervisor(factory);

    let handle = supervisor
        .delegate(DelegateAgentRequest {
            template_id: "explorer".to_string(),
            task: "inspect src/lib.rs".to_string(),
            overrides: AgentOverrides::empty(),
            parent_session_id: Some("parent-session".to_string()),
        })
        .unwrap();
    let report = supervisor.join(&handle.id).unwrap();

    assert!(report.contains("## Summary"));
    assert!(report.contains("answer"));
    assert_eq!(
        supervisor.status(&handle.id).unwrap(),
        AgentStatus::Completed
    );
    assert_eq!(supervisor.join(&handle.id).unwrap(), report);

    let agent_root = fixture.agent_root(&handle.id);
    assert_file_contains(&agent_root.join("report.md"), "## Evidence");
    assert_file_contains(&agent_root.join("events.jsonl"), "spawned read_only");
    assert_file_contains(&agent_root.join("events.jsonl"), "completed");
    assert_file_contains(&agent_root.join("history.jsonl"), "call_read");
    assert_file_contains(&agent_root.join("history.jsonl"), "pub fn answer");

    let calls = trace.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].tool_names, vec!["grep", "load_skill", "read_file"]);
    assert!(calls[0].messages[0].content.contains("# Available Skills"));
    assert!(calls[0].messages[0].content.contains("id: subagents"));
    assert!(
        !calls[0].messages[0]
            .content
            .contains("# Subagents Test Skill")
    );
    assert!(calls[0].messages.iter().any(|message| {
        message.content.contains("explorer subagent")
            && message.content.contains("# Task")
            && message.content.contains("inspect src/lib.rs")
            && message.content.contains("# Output Contract")
    }));
    assert!(calls[1].messages.iter().any(|message| {
        message.tool_call_id.as_deref() == Some("call_read")
            && message.content.contains("pub fn answer")
    }));
}

#[test]
fn read_only_subagent_profile_hides_mutating_tools_from_provider() {
    let fixture = SupervisorFixture::new();
    let factory = Arc::new(ScriptedProviderFactory::new(vec![Ok(Message::assistant(
        "## Summary\n\nNo mutation tools available.\n\n## Evidence\n\n- Tool schema checked.\n\n## Uncertainty\n\nNone",
    ))]));
    let trace = factory.trace();
    let supervisor =
        fixture.supervisor_with_registry(factory, fixture.registry_with_mutating_tool());

    let handle = supervisor
        .delegate(DelegateAgentRequest {
            template_id: "explorer".to_string(),
            task: "list tools".to_string(),
            overrides: AgentOverrides::empty(),
            parent_session_id: None,
        })
        .unwrap();

    supervisor.join(&handle.id).unwrap();
    let tool_names = &trace.calls.lock().unwrap()[0].tool_names;
    assert_eq!(tool_names, &vec!["grep", "load_skill", "read_file"]);
    assert!(!tool_names.iter().any(|name| name == "write_file"));
}

#[test]
fn missing_profile_tool_fails_before_creating_agent() {
    let fixture = SupervisorFixture::new();
    let factory = Arc::new(ScriptedProviderFactory::new(Vec::new()));
    let mut registry = ToolRegistry::new();
    registry
        .register(ReadFileTool::new(fixture.work_dir.path()).unwrap())
        .unwrap();
    registry
        .register(
            LoadSkillTool::new(fixture.work_dir.path(), vec!["subagents".to_string()]).unwrap(),
        )
        .unwrap();
    let supervisor = fixture.supervisor_with_registry(factory, registry);

    let error = supervisor
        .delegate(DelegateAgentRequest {
            template_id: "explorer".to_string(),
            task: "inspect".to_string(),
            overrides: AgentOverrides::empty(),
            parent_session_id: None,
        })
        .unwrap_err();

    assert!(error.to_string().contains("unknown tool: grep"));
    assert!(!fixture.work_dir.path().join(".tiny-claw/agents").exists());
}

#[test]
fn unknown_template_fails_before_creating_agent() {
    let fixture = SupervisorFixture::new();
    let supervisor = fixture.supervisor(Arc::new(ScriptedProviderFactory::new(Vec::new())));

    let error = supervisor
        .delegate(DelegateAgentRequest {
            template_id: "missing".to_string(),
            task: "inspect".to_string(),
            overrides: AgentOverrides::empty(),
            parent_session_id: None,
        })
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("unknown subagent template: missing")
    );
    assert!(!fixture.work_dir.path().join(".tiny-claw/agents").exists());
}

#[test]
fn provider_create_failure_marks_agent_failed_and_join_reports_error() {
    let fixture = SupervisorFixture::new();
    let supervisor = fixture.supervisor(Arc::new(FailingProviderFactory));

    let handle = supervisor
        .delegate(DelegateAgentRequest {
            template_id: "explorer".to_string(),
            task: "inspect".to_string(),
            overrides: AgentOverrides::empty(),
            parent_session_id: None,
        })
        .unwrap();
    let error = supervisor.join(&handle.id).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("agent failed: provider unavailable")
    );
    assert_eq!(supervisor.status(&handle.id).unwrap(), AgentStatus::Failed);
    assert_file_contains(
        &fixture.agent_root(&handle.id).join("events.jsonl"),
        "failed: provider unavailable",
    );
}

#[test]
fn empty_final_message_marks_agent_failed() {
    let fixture = SupervisorFixture::new();
    let supervisor = fixture.supervisor(Arc::new(ScriptedProviderFactory::new(vec![Ok(
        Message::assistant("   "),
    )])));

    let handle = supervisor
        .delegate(DelegateAgentRequest {
            template_id: "explorer".to_string(),
            task: "inspect".to_string(),
            overrides: AgentOverrides::empty(),
            parent_session_id: None,
        })
        .unwrap();
    let error = supervisor.join(&handle.id).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("subagent completed without final content")
    );
    assert_eq!(supervisor.status(&handle.id).unwrap(), AgentStatus::Failed);
}

#[test]
fn unknown_agent_errors_are_explicit() {
    let fixture = SupervisorFixture::new();
    let supervisor = fixture.supervisor(Arc::new(ScriptedProviderFactory::new(Vec::new())));

    assert_eq!(
        supervisor.status("agent_missing").unwrap_err().to_string(),
        "unknown agent: agent_missing"
    );
    assert_eq!(
        supervisor.join("agent_missing").unwrap_err().to_string(),
        "unknown agent: agent_missing"
    );
    assert_eq!(
        supervisor.cancel("agent_missing").unwrap_err().to_string(),
        "unknown agent: agent_missing"
    );
}

#[test]
fn cancel_running_agent_reports_cancelled_on_join() {
    let fixture = SupervisorFixture::new();
    let supervisor = fixture.supervisor(Arc::new(ScriptedProviderFactory::new(vec![Ok(
        Message::assistant("## Summary\n\nLate report"),
    )])));

    let handle = supervisor
        .delegate(DelegateAgentRequest {
            template_id: "explorer".to_string(),
            task: "inspect".to_string(),
            overrides: AgentOverrides::empty(),
            parent_session_id: None,
        })
        .unwrap();

    assert_eq!(
        supervisor.cancel(&handle.id).unwrap(),
        AgentStatus::Cancelled
    );
    assert_eq!(
        supervisor.join(&handle.id).unwrap_err().to_string(),
        "agent was cancelled"
    );
}

#[test]
fn runtime_command_parsing_and_output_use_tool_results() {
    let fixture = SupervisorFixture::new();
    let supervisor = fixture.supervisor(Arc::new(ScriptedProviderFactory::new(vec![Ok(
        Message::assistant("## Summary\n\nDone\n\n## Evidence\n\n- ok\n\n## Uncertainty\n\nNone"),
    )])));

    let delegate = supervisor.execute_command(&ToolCall::new(
        "call_delegate",
        "delegate_agent",
        json!({ "template_id": "explorer", "task": "inspect" }),
    ));
    assert!(!delegate.is_error);
    assert!(delegate.output.contains("agent_id: agent_001"));
    assert!(delegate.output.contains("status: running"));

    let status = supervisor.execute_command(&ToolCall::new(
        "call_status",
        "agent_status",
        json!({ "agent_id": "agent_001" }),
    ));
    assert!(!status.is_error);
    assert!(status.output.contains("agent_id: agent_001"));

    let joined = supervisor.execute_command(&ToolCall::new(
        "call_join",
        "join_agent",
        json!({ "agent_id": "agent_001" }),
    ));
    assert!(!joined.is_error);
    assert!(joined.output.contains("## Summary"));

    let missing_arg = supervisor.execute_command(&ToolCall::new(
        "call_bad",
        "delegate_agent",
        json!({ "template_id": "explorer" }),
    ));
    assert!(missing_arg.is_error);
    assert!(missing_arg.output.contains("missing string argument: task"));
}

struct SupervisorFixture {
    work_dir: TempDir,
}

impl SupervisorFixture {
    fn new() -> Self {
        let work_dir = tempdir().unwrap();
        fs::create_dir_all(work_dir.path().join("src")).unwrap();
        fs::write(
            work_dir.path().join("src/lib.rs"),
            "pub fn answer() -> u8 { 42 }\n",
        )
        .unwrap();
        write_skill(
            work_dir.path(),
            "subagents",
            "# Subagents Test Skill\nUse evidence.\n",
        );

        Self { work_dir }
    }

    fn supervisor(&self, factory: Arc<dyn ProviderFactory>) -> AgentSupervisor {
        self.supervisor_with_registry(factory, self.registry())
    }

    fn supervisor_with_registry(
        &self,
        factory: Arc<dyn ProviderFactory>,
        registry: ToolRegistry,
    ) -> AgentSupervisor {
        AgentSupervisor::new(
            factory,
            registry,
            self.work_dir.path(),
            self.work_dir.path().join(".tiny-claw"),
        )
    }

    fn registry(&self) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry
            .register(ReadFileTool::new(self.work_dir.path()).unwrap())
            .unwrap();
        registry
            .register(GrepTool::new(self.work_dir.path()).unwrap())
            .unwrap();
        registry
            .register(
                LoadSkillTool::new(self.work_dir.path(), vec!["subagents".to_string()]).unwrap(),
            )
            .unwrap();
        registry
    }

    fn registry_with_mutating_tool(&self) -> ToolRegistry {
        let mut registry = self.registry();
        registry.register(MutatingTestTool).unwrap();
        registry
    }

    fn agent_root(&self, agent_id: &str) -> std::path::PathBuf {
        self.work_dir
            .path()
            .join(".tiny-claw")
            .join("agents")
            .join(agent_id)
    }
}

fn write_skill(work_dir: &Path, skill_id: &str, body: &str) {
    let skill_dir = work_dir.join(".tiny-claw").join("skills").join(skill_id);
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), body).unwrap();
}

fn assert_file_contains(path: &Path, needle: &str) {
    let content = fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("failed to read {}: {error}", path.display());
    });
    assert!(
        content.contains(needle),
        "expected {} to contain {needle:?}, got:\n{content}",
        path.display()
    );
}
