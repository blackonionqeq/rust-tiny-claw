use crate::context_engine::ContextManager;
use crate::engine::AgentEngine;
use crate::memory::FileMemory;
use crate::provider::{ClaudeCompatibleProvider, MockProvider, OpenAiCompatibleProvider, Provider};
use crate::telemetry::Telemetry;
use crate::tools::{BashTool, EditFileTool, GrepTool, ReadFileTool, ToolRegistry, WriteFileTool};
use std::env;
use std::path::Path;

pub fn build_engine(
    work_dir: &Path,
) -> Result<AgentEngine<Box<dyn Provider>>, Box<dyn std::error::Error>> {
    let provider = build_provider()?;
    let active_skills = active_skills_from_env();

    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool::new(work_dir)?)?;
    registry.register(WriteFileTool::new(work_dir)?)?;
    registry.register(BashTool::new(work_dir)?)?;
    registry.register(EditFileTool::new(work_dir)?)?;
    registry.register(GrepTool::new(work_dir)?)?;

    Ok(AgentEngine::new(
        provider,
        registry,
        ContextManager::new(work_dir, active_skills),
        FileMemory::new(".tiny-claw"),
        Telemetry::default(),
    ))
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
        })
        .collect()
}

fn build_provider() -> Result<Box<dyn Provider>, Box<dyn std::error::Error>> {
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
