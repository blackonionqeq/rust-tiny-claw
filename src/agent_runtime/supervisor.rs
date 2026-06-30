use super::templates::{
    AgentOverrides, AgentSpec, SubagentTemplateRegistry, TemplateError, ToolProfileRegistry,
};
use crate::context_engine::ContextManager;
use crate::engine::{AgentEngine, RunOptions};
use crate::memory::{FileMemory, Session};
use crate::provider::{ProviderError, ProviderFactory};
use crate::schema::{Message, ToolCall, ToolDefinition, ToolResult};
use crate::telemetry::Telemetry;
use crate::tools::ToolRegistry;
use serde_json::json;
use std::collections::HashMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub type AgentId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHandle {
    pub id: AgentId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl fmt::Display for AgentStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(formatter, "running"),
            Self::Completed => write!(formatter, "completed"),
            Self::Failed => write!(formatter, "failed"),
            Self::Cancelled => write!(formatter, "cancelled"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegateAgentRequest {
    pub template_id: String,
    pub task: String,
    pub overrides: AgentOverrides,
    pub parent_session_id: Option<String>,
}

#[derive(Clone)]
pub struct AgentSupervisor {
    inner: Arc<SupervisorInner>,
}

struct SupervisorInner {
    provider_factory: Arc<dyn ProviderFactory>,
    templates: SubagentTemplateRegistry,
    tool_profiles: ToolProfileRegistry,
    base_registry: ToolRegistry,
    work_dir: PathBuf,
    memory_root: PathBuf,
    agents: Mutex<HashMap<AgentId, AgentRecord>>,
    next_id: AtomicUsize,
}

struct AgentRecord {
    status: Arc<Mutex<AgentRunStatus>>,
    cancel_requested: Arc<AtomicBool>,
    join: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Debug, Clone)]
struct AgentRunStatus {
    status: AgentStatus,
    report: Option<String>,
    error: Option<String>,
}

impl AgentSupervisor {
    pub fn new(
        provider_factory: Arc<dyn ProviderFactory>,
        base_registry: ToolRegistry,
        work_dir: impl Into<PathBuf>,
        memory_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(SupervisorInner {
                provider_factory,
                templates: SubagentTemplateRegistry::built_in(),
                tool_profiles: ToolProfileRegistry::built_in(),
                base_registry,
                work_dir: work_dir.into(),
                memory_root: memory_root.into(),
                agents: Mutex::new(HashMap::new()),
                next_id: AtomicUsize::new(1),
            }),
        }
    }

    pub fn runtime_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition::new(
                "delegate_agent",
                format!(
                    "Delegate bounded investigation work to a subagent. Available templates:\n{}",
                    self.inner.templates.describe_templates()
                ),
                json!({
                    "type": "object",
                    "properties": {
                        "template_id": { "type": "string", "description": "Subagent template id, such as explorer." },
                        "task": { "type": "string", "description": "Concrete investigation request for the subagent." },
                        "role_prompt_append": { "type": "string", "description": "Optional safe role guidance append; does not grant new tools or skills." }
                    },
                    "required": ["template_id", "task"]
                }),
            ),
            ToolDefinition::new(
                "agent_status",
                "Return the lifecycle status for a delegated subagent.",
                json!({
                    "type": "object",
                    "properties": { "agent_id": { "type": "string" } },
                    "required": ["agent_id"]
                }),
            ),
            ToolDefinition::new(
                "join_agent",
                "Wait for a delegated subagent and return only its final report.",
                json!({
                    "type": "object",
                    "properties": { "agent_id": { "type": "string" } },
                    "required": ["agent_id"]
                }),
            ),
            ToolDefinition::new(
                "cancel_agent",
                "Request cooperative cancellation for a delegated subagent.",
                json!({
                    "type": "object",
                    "properties": { "agent_id": { "type": "string" } },
                    "required": ["agent_id"]
                }),
            ),
        ]
    }

    pub fn is_runtime_command(name: &str) -> bool {
        matches!(
            name,
            "delegate_agent" | "agent_status" | "join_agent" | "cancel_agent"
        )
    }

    pub fn execute_command(&self, call: &ToolCall) -> ToolResult {
        let result = match call.name.as_str() {
            "delegate_agent" => self.delegate_from_call(call),
            "agent_status" => self.agent_status_from_call(call),
            "join_agent" => self.join_from_call(call),
            "cancel_agent" => self.cancel_from_call(call),
            _ => Err(RuntimeCommandError::UnknownCommand(call.name.clone())),
        };

        match result {
            Ok(output) => ToolResult::ok(call.id.clone(), output),
            Err(error) => ToolResult::error(call.id.clone(), error.to_string()),
        }
    }

    pub fn delegate(
        &self,
        request: DelegateAgentRequest,
    ) -> Result<AgentHandle, RuntimeCommandError> {
        let spec = self.inner.templates.resolve(
            &request.template_id,
            request.task,
            request.overrides,
            request.parent_session_id,
        )?;
        self.spawn(spec)
    }

    pub fn status(&self, agent_id: &str) -> Result<AgentStatus, RuntimeCommandError> {
        let agents = self.inner.agents.lock().expect("agent table lock poisoned");
        let record = agents
            .get(agent_id)
            .ok_or_else(|| RuntimeCommandError::UnknownAgent(agent_id.to_string()))?;
        Ok(record
            .status
            .lock()
            .expect("agent status lock poisoned")
            .status
            .clone())
    }

    pub fn join(&self, agent_id: &str) -> Result<String, RuntimeCommandError> {
        let join = {
            let agents = self.inner.agents.lock().expect("agent table lock poisoned");
            let record = agents
                .get(agent_id)
                .ok_or_else(|| RuntimeCommandError::UnknownAgent(agent_id.to_string()))?;
            record.join.lock().expect("agent join lock poisoned").take()
        };

        if let Some(join) = join {
            join.join().map_err(|_| {
                RuntimeCommandError::AgentFailed("agent thread panicked".to_string())
            })?;
        }

        let agents = self.inner.agents.lock().expect("agent table lock poisoned");
        let record = agents
            .get(agent_id)
            .ok_or_else(|| RuntimeCommandError::UnknownAgent(agent_id.to_string()))?;
        let status = record.status.lock().expect("agent status lock poisoned");
        match status.status {
            AgentStatus::Completed => status
                .report
                .clone()
                .filter(|report| !report.trim().is_empty())
                .ok_or(RuntimeCommandError::EmptyReport),
            AgentStatus::Failed => Err(RuntimeCommandError::AgentFailed(
                status
                    .error
                    .clone()
                    .unwrap_or_else(|| "agent failed".to_string()),
            )),
            AgentStatus::Cancelled => Err(RuntimeCommandError::AgentCancelled),
            AgentStatus::Running => Err(RuntimeCommandError::AgentStillRunning),
        }
    }

    pub fn cancel(&self, agent_id: &str) -> Result<AgentStatus, RuntimeCommandError> {
        let agents = self.inner.agents.lock().expect("agent table lock poisoned");
        let record = agents
            .get(agent_id)
            .ok_or_else(|| RuntimeCommandError::UnknownAgent(agent_id.to_string()))?;
        record.cancel_requested.store(true, Ordering::SeqCst);
        let mut status = record.status.lock().expect("agent status lock poisoned");
        if status.status == AgentStatus::Running {
            status.status = AgentStatus::Cancelled;
            status.error = Some("cancel requested".to_string());
        }
        Ok(status.status.clone())
    }

    fn spawn(&self, spec: AgentSpec) -> Result<AgentHandle, RuntimeCommandError> {
        let tool_names = self.inner.tool_profiles.tools_for(&spec.tool_profile)?;
        let registry = self
            .inner
            .base_registry
            .subset(&tool_names)
            .map_err(RuntimeCommandError::ToolProfile)?;

        let id = format!(
            "agent_{:03}",
            self.inner.next_id.fetch_add(1, Ordering::SeqCst)
        );
        let status = Arc::new(Mutex::new(AgentRunStatus {
            status: AgentStatus::Running,
            report: None,
            error: None,
        }));
        let cancel_requested = Arc::new(AtomicBool::new(false));
        let inner = Arc::clone(&self.inner);
        let thread_status = Arc::clone(&status);
        let thread_cancel = Arc::clone(&cancel_requested);
        let thread_id = id.clone();
        let join = std::thread::spawn(move || {
            run_agent_thread(
                inner,
                thread_id,
                spec,
                registry,
                thread_status,
                thread_cancel,
            );
        });

        let record = AgentRecord {
            status,
            cancel_requested,
            join: Mutex::new(Some(join)),
        };
        self.inner
            .agents
            .lock()
            .expect("agent table lock poisoned")
            .insert(id.clone(), record);

        Ok(AgentHandle { id })
    }

    fn delegate_from_call(&self, call: &ToolCall) -> Result<String, RuntimeCommandError> {
        let template_id = required_string(call, "template_id")?.to_string();
        let task = required_string(call, "task")?.to_string();
        let mut overrides = AgentOverrides::empty();
        overrides.role_prompt_append = call
            .arguments
            .get("role_prompt_append")
            .and_then(|value| value.as_str())
            .map(str::to_string);

        let handle = self.delegate(DelegateAgentRequest {
            template_id,
            task,
            overrides,
            parent_session_id: None,
        })?;

        Ok(format!(
            "agent_id: {}\nstatus: running\nUse join_agent when you need the final report.",
            handle.id
        ))
    }

    fn agent_status_from_call(&self, call: &ToolCall) -> Result<String, RuntimeCommandError> {
        let agent_id = required_string(call, "agent_id")?;
        Ok(format!(
            "agent_id: {agent_id}\nstatus: {}",
            self.status(agent_id)?
        ))
    }

    fn join_from_call(&self, call: &ToolCall) -> Result<String, RuntimeCommandError> {
        let agent_id = required_string(call, "agent_id")?;
        self.join(agent_id)
    }

    fn cancel_from_call(&self, call: &ToolCall) -> Result<String, RuntimeCommandError> {
        let agent_id = required_string(call, "agent_id")?;
        Ok(format!(
            "agent_id: {agent_id}\nstatus: {}",
            self.cancel(agent_id)?
        ))
    }
}

fn run_agent_thread(
    inner: Arc<SupervisorInner>,
    agent_id: AgentId,
    spec: AgentSpec,
    registry: ToolRegistry,
    status: Arc<Mutex<AgentRunStatus>>,
    cancel_requested: Arc<AtomicBool>,
) {
    let agent_root = inner.memory_root.join("agents").join(&agent_id);
    let _ = fs::create_dir_all(&agent_root);
    append_event(&agent_root, &format!("spawned {}", spec.tool_profile));

    if cancel_requested.load(Ordering::SeqCst) {
        mark_cancelled(&agent_root, &status);
        return;
    }

    let provider = match inner.provider_factory.create() {
        Ok(provider) => provider,
        Err(error) => {
            mark_failed(&agent_root, &status, error.to_string());
            return;
        }
    };

    let task = format!(
        "{}\n\n# Task\n\n{}\n\n# Output Contract\n\nReturn Markdown with these sections:\n\n## Summary\n\n## Evidence\n\n## Uncertainty",
        spec.system_prompt, spec.task
    );
    let session = Session::new(agent_id.clone(), inner.work_dir.clone());
    let mut engine = AgentEngine::new(
        provider,
        registry,
        ContextManager::new(&inner.work_dir, spec.skills),
        FileMemory::new(inner.memory_root.clone()),
        Telemetry::default(),
    );
    let options = RunOptions {
        stream: false,
        context_budget: spec.context_budget,
        ..RunOptions::default()
    };

    let result = engine.run_session(&session, task, options);
    persist_history(&agent_root, &session.history());

    if cancel_requested.load(Ordering::SeqCst) {
        mark_cancelled(&agent_root, &status);
        return;
    }

    match result {
        Ok(transcript) => {
            let report = transcript
                .iter()
                .rev()
                .find(|message| message.tool_calls.is_empty() && message.tool_call_id.is_none())
                .map(|message| message.content.clone())
                .unwrap_or_default();
            if report.trim().is_empty() {
                mark_failed(
                    &agent_root,
                    &status,
                    "subagent completed without final content".to_string(),
                );
                return;
            }
            let _ = fs::write(agent_root.join("report.md"), &report);
            append_event(&agent_root, "completed");
            let mut status = status.lock().expect("agent status lock poisoned");
            status.status = AgentStatus::Completed;
            status.report = Some(report);
        }
        Err(error) => mark_failed(&agent_root, &status, error.to_string()),
    }
}

fn persist_history(agent_root: &std::path::Path, history: &[Message]) {
    let path = agent_root.join("history.jsonl");
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    for message in history {
        let line = json!({
            "role": format!("{:?}", message.role),
            "content": message.content,
            "tool_call_id": message.tool_call_id,
            "tool_calls": message.tool_calls.iter().map(|call| {
                json!({ "id": call.id, "name": call.name, "arguments": call.arguments })
            }).collect::<Vec<_>>()
        });
        let _ = writeln!(file, "{line}");
    }
}

fn append_event(agent_root: &std::path::Path, event: &str) {
    let _ = fs::create_dir_all(agent_root);
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(agent_root.join("events.jsonl"))
    {
        let _ = writeln!(file, "{}", json!({ "event": event }));
    }
}

fn mark_failed(agent_root: &std::path::Path, status: &Arc<Mutex<AgentRunStatus>>, error: String) {
    append_event(agent_root, &format!("failed: {error}"));
    let mut status = status.lock().expect("agent status lock poisoned");
    status.status = AgentStatus::Failed;
    status.error = Some(error);
}

fn mark_cancelled(agent_root: &std::path::Path, status: &Arc<Mutex<AgentRunStatus>>) {
    append_event(agent_root, "cancelled");
    let mut status = status.lock().expect("agent status lock poisoned");
    status.status = AgentStatus::Cancelled;
    status.error = Some("cancelled".to_string());
}

fn required_string<'a>(call: &'a ToolCall, name: &str) -> Result<&'a str, RuntimeCommandError> {
    call.arguments
        .get(name)
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            RuntimeCommandError::InvalidArguments(format!("missing string argument: {name}"))
        })
}

#[derive(Debug)]
pub enum RuntimeCommandError {
    UnknownCommand(String),
    UnknownAgent(String),
    InvalidArguments(String),
    ToolProfile(String),
    Template(TemplateError),
    Provider(ProviderError),
    AgentFailed(String),
    AgentCancelled,
    AgentStillRunning,
    EmptyReport,
}

impl fmt::Display for RuntimeCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(command) => {
                write!(formatter, "unknown runtime command: {command}")
            }
            Self::UnknownAgent(agent_id) => write!(formatter, "unknown agent: {agent_id}"),
            Self::InvalidArguments(message) => write!(formatter, "{message}"),
            Self::ToolProfile(message) => write!(formatter, "{message}"),
            Self::Template(error) => write!(formatter, "{error}"),
            Self::Provider(error) => write!(formatter, "{error}"),
            Self::AgentFailed(error) => write!(formatter, "agent failed: {error}"),
            Self::AgentCancelled => write!(formatter, "agent was cancelled"),
            Self::AgentStillRunning => write!(formatter, "agent is still running"),
            Self::EmptyReport => write!(formatter, "agent completed without a final report"),
        }
    }
}

impl std::error::Error for RuntimeCommandError {}

impl From<TemplateError> for RuntimeCommandError {
    fn from(error: TemplateError) -> Self {
        Self::Template(error)
    }
}

impl From<ProviderError> for RuntimeCommandError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

#[cfg(test)]
mod tests;
