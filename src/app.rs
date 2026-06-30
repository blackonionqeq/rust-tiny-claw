use crate::agent_runtime::AgentSupervisor;
use crate::context_engine::ContextManager;
use crate::engine::AgentEngine;
#[cfg(feature = "feishu")]
use crate::integrations::feishu::approval::ApprovalManager;
#[cfg(feature = "feishu")]
use crate::integrations::feishu::client::FeishuClient;
use crate::memory::FileMemory;
use crate::provider::{
    ClaudeCompatibleProvider, MockProvider, OpenAiCompatibleProvider, Provider, ProviderError,
    ProviderFactory,
};
use crate::telemetry::Telemetry;
use crate::tools::{
    BashTool, EditFileTool, GrepTool, LoadSkillTool, ReadFileTool, ToolRegistry, WriteFileTool,
};
#[cfg(feature = "feishu")]
use crate::tools::{PermissionDecision, RuleBasedToolPolicy, ToolPolicy};
use std::env;
use std::path::Path;
use std::sync::Arc;

pub fn build_engine(
    work_dir: &Path,
) -> Result<AgentEngine<Box<dyn Provider + Send>>, Box<dyn std::error::Error>> {
    let work_dir = work_dir.canonicalize()?;
    let provider_factory = Arc::new(EnvProviderFactory);
    let provider = provider_factory.create()?;
    let active_skills = active_skills_from_env();
    let registry = build_registry(&work_dir, active_skills.clone())?;
    let supervisor = AgentSupervisor::new(
        provider_factory,
        registry.clone(),
        work_dir.clone(),
        work_dir.join(".tiny-claw"),
    );

    Ok(AgentEngine::new(
        provider,
        registry,
        ContextManager::new(&work_dir, active_skills),
        FileMemory::new(work_dir.join(".tiny-claw")),
        Telemetry::default(),
    )
    .with_supervisor(supervisor))
}

#[cfg(feature = "feishu")]
pub fn build_feishu_engine(
    work_dir: &Path,
    client: FeishuClient,
    chat_id: String,
    approval_manager: Arc<ApprovalManager>,
) -> Result<AgentEngine<Box<dyn Provider + Send>>, Box<dyn std::error::Error>> {
    let work_dir = work_dir.canonicalize()?;
    let provider = build_provider()?;
    let active_skills = active_skills_from_env();
    let mut registry = build_registry(&work_dir, active_skills.clone())?;
    let policy = Arc::new(RuleBasedToolPolicy::default());

    registry.use_middleware(
        move |call: &crate::schema::ToolCall| match policy.decide(call) {
            PermissionDecision::Allow => None,
            PermissionDecision::Deny { reason } => Some(crate::schema::ToolResult::error(
                call.id.clone(),
                format!("Tool call denied by policy: {reason}"),
            )),
            PermissionDecision::Ask { reason } => {
                match approval_manager.wait_for_tool_approval(&client, &chat_id, call, &reason) {
                    Ok(resolution) if resolution.allowed => None,
                    Ok(resolution) => Some(crate::schema::ToolResult::error(
                        call.id.clone(),
                        resolution.reason,
                    )),
                    Err(error) => Some(crate::schema::ToolResult::error(
                        call.id.clone(),
                        format!("Tool call requires approval, but approval failed: {error}"),
                    )),
                }
            }
        },
    );

    Ok(AgentEngine::new(
        provider,
        registry,
        ContextManager::new(&work_dir, active_skills),
        FileMemory::new(work_dir.join(".tiny-claw")),
        Telemetry::default(),
    ))
}

fn build_registry(
    work_dir: &Path,
    active_skills: Vec<String>,
) -> Result<ToolRegistry, Box<dyn std::error::Error>> {
    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool::new(work_dir)?)?;
    registry.register(LoadSkillTool::new(work_dir, active_skills)?)?;
    registry.register(WriteFileTool::new(work_dir)?)?;
    registry.register(BashTool::new(work_dir)?)?;
    registry.register(EditFileTool::new(work_dir)?)?;
    registry.register(GrepTool::new(work_dir)?)?;
    Ok(registry)
}

pub fn stream_enabled() -> Result<bool, Box<dyn std::error::Error>> {
    match env::var("TINY_CLAW_STREAM") {
        Ok(value) => parse_bool_env("TINY_CLAW_STREAM", &value),
        Err(_) => Ok(true),
    }
}

fn parse_bool_env(name: &str, value: &str) -> Result<bool, Box<dyn std::error::Error>> {
    match value {
        "1" | "true" | "TRUE" | "True" | "yes" | "YES" | "Yes" => Ok(true),
        "0" | "false" | "FALSE" | "False" | "no" | "NO" | "No" => Ok(false),
        _ => Err(format!("invalid {name} value: {value}").into()),
    }
}

fn active_skills_from_env() -> Vec<String> {
    let mut skills = vec!["subagents".to_string()];
    skills.extend(
        env::var("TINY_CLAW_SKILLS")
            .ok()
            .into_iter()
            .flat_map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|skill| !skill.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            }),
    );
    skills.sort();
    skills.dedup();
    skills
}

fn build_provider() -> Result<Box<dyn Provider + Send>, Box<dyn std::error::Error>> {
    match env::var("TINY_CLAW_PROVIDER")
        .unwrap_or_else(|_| "mock".to_string())
        .as_str()
    {
        "mock" => Ok(Box::new(MockProvider::default())),
        "claude-compatible" => Ok(Box::new(ClaudeCompatibleProvider::from_env()?)),
        "openai-compatible" => Ok(Box::new(OpenAiCompatibleProvider::from_env()?)),
        other => Err(format!("unsupported TINY_CLAW_PROVIDER: {other}").into()),
    }
}

struct EnvProviderFactory;

impl ProviderFactory for EnvProviderFactory {
    fn create(&self) -> Result<Box<dyn Provider + Send>, ProviderError> {
        build_provider().map_err(|error| ProviderError::new(error.to_string()))
    }
}
