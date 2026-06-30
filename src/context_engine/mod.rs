mod compaction;
mod recovery;
mod reminder;
mod skills;

pub use compaction::{ContextBudget, ContextCompactor};
pub use recovery::{RecoveryAdvice, RecoveryCode, RecoveryManager};
pub use reminder::ReminderManager;
use skills::load_active_skill_manifests;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

pub use skills::{SkillDocument, SkillManifest, load_model_invokable_skill};

const BASE_INSTRUCTIONS: &str =
    "You are rust-tiny-claw, a small coding assistant running inside one workspace.";

const SUBAGENT_DELEGATION_INSTRUCTIONS: &str = r#"# Subagent Delegation

You may delegate bounded investigation work to subagents when broad exploration would pollute the main context. Use subagents for multi-file or uncertain exploration that can be summarized as evidence. Do not use subagents for small known reads, final decisions, or workspace mutations.

When you need detailed subagent templates, delegation rules, waiting behavior, or examples, load the `subagents` skill."#;

// Plan Mode is prompt-only state externalization: the engine stays stateless
// about task plans and lets the model maintain PLAN.md/TODO.md through tools.
const PLAN_MODE_INSTRUCTIONS: &str = r#"# Plan Mode

Plan Mode is enabled for this run. Treat this as a long-running task that may outlive the current process and context window.

Use workspace files as externalized task state:

1. At the start of the task, inspect the workspace root for `PLAN.md` and `TODO.md`.
2. If they do not exist, create `PLAN.md` with the overall goal, constraints, and approach, then create `TODO.md` with concrete Markdown checklist items.
3. If they already exist, do not overwrite them. Read both files, use `PLAN.md` for the current strategy, and continue from the first unchecked `- [ ]` item in `TODO.md`.
4. After completing a checklist item, immediately update `TODO.md` from `- [ ]` to `- [x]` for that item before moving on.
5. If you lose track of the task or hit an error, reread `TODO.md` and continue from the next unchecked item.

Keep `PLAN.md` and `TODO.md` concise and useful for human review. Simple one-off requests do not need extra files unless Plan Mode is enabled."#;

#[derive(Debug, Clone)]
pub struct ContextManager {
    work_dir: PathBuf,
    active_skills: Vec<String>,
}

impl ContextManager {
    pub fn new(work_dir: impl Into<PathBuf>, active_skills: Vec<String>) -> Self {
        Self {
            work_dir: work_dir.into(),
            active_skills,
        }
    }

    pub fn name(&self) -> &'static str {
        "context-manager"
    }

    pub fn work_dir(&self) -> &PathBuf {
        &self.work_dir
    }

    pub fn build_system_prompt(&self, plan_mode: bool) -> Result<String, ContextError> {
        let mut sections = vec![format!("# Base Instructions\n\n{BASE_INSTRUCTIONS}")];
        sections.push(SUBAGENT_DELEGATION_INSTRUCTIONS.to_string());

        if plan_mode {
            sections.push(PLAN_MODE_INSTRUCTIONS.to_string());
        }

        if let Some(workspace_instructions) = self.load_workspace_instructions()? {
            sections.push(format!(
                "# Workspace Instructions\n\nThe following instructions were loaded from AGENTS.md.\n\n{}",
                workspace_instructions.trim()
            ));
        }

        let skills = load_active_skill_manifests(&self.work_dir, &self.active_skills)?;
        let model_invokable_skills = skills
            .into_iter()
            .filter(|skill| !skill.disable_model_invocation)
            .collect::<Vec<_>>();
        if !model_invokable_skills.is_empty() {
            let mut rendered = String::from(
                "# Available Skills\n\nThe following enabled skills can be loaded when relevant. To use one, call load_skill with its id.",
            );
            for skill in model_invokable_skills {
                rendered.push_str(&format!(
                    "\n\n- id: {}\n  name: {}\n  source: {}",
                    skill.id,
                    skill.name,
                    skill.source.display()
                ));
                if let Some(description) = skill.description
                    && !description.is_empty()
                {
                    rendered.push_str(&format!("\n  description: {description}"));
                }
            }
            sections.push(rendered);
        }

        Ok(sections.join("\n\n"))
    }

    fn load_workspace_instructions(&self) -> Result<Option<String>, ContextError> {
        let path = self.work_dir.join("AGENTS.md");
        match fs::read_to_string(&path) {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ContextError::ReadFile {
                path,
                source: error,
            }),
        }
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new(PathBuf::from("."), Vec::new())
    }
}

#[derive(Debug)]
pub enum ContextError {
    InvalidSkillId(String),
    InvalidSkillMetadata { path: PathBuf, message: String },
    ReadFile { path: PathBuf, source: io::Error },
    SkillModelInvocationDisabled(String),
    SkillNotEnabled(String),
}

impl fmt::Display for ContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSkillId(skill_id) => write!(formatter, "invalid skill id: {skill_id}"),
            Self::InvalidSkillMetadata { path, message } => {
                write!(
                    formatter,
                    "invalid skill metadata in {}: {message}",
                    path.display()
                )
            }
            Self::ReadFile { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::SkillModelInvocationDisabled(skill_id) => {
                write!(
                    formatter,
                    "skill '{skill_id}' is disabled for model invocation"
                )
            }
            Self::SkillNotEnabled(skill_id) => {
                write!(formatter, "skill '{skill_id}' is not enabled")
            }
        }
    }
}

impl std::error::Error for ContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidSkillId(_)
            | Self::InvalidSkillMetadata { .. }
            | Self::SkillModelInvocationDisabled(_)
            | Self::SkillNotEnabled(_) => None,
            Self::ReadFile { source, .. } => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn base_prompt_renders_without_workspace_or_skills() {
        let work_dir = tempdir().unwrap();

        let prompt = ContextManager::new(work_dir.path(), Vec::new())
            .build_system_prompt(false)
            .unwrap();

        assert!(prompt.contains("# Base Instructions"));
        assert!(prompt.contains(BASE_INSTRUCTIONS));
        assert!(!prompt.contains("# Workspace Instructions"));
        assert!(!prompt.contains("# Active Skills"));
    }

    #[test]
    fn agents_md_content_appears_in_workspace_section() {
        let work_dir = tempdir().unwrap();
        fs::write(
            work_dir.path().join("AGENTS.md"),
            "Use project conventions.\n",
        )
        .unwrap();

        let prompt = ContextManager::new(work_dir.path(), Vec::new())
            .build_system_prompt(false)
            .unwrap();

        assert!(prompt.contains("# Workspace Instructions"));
        assert!(prompt.contains("Use project conventions."));
    }

    #[test]
    fn active_skills_render_catalog_in_requested_order() {
        let work_dir = tempdir().unwrap();
        write_skill(
            work_dir.path(),
            "rust",
            "---\nname: Rust\ndescription: Rust workflows\n---\n\n# Rust Skill\n",
        );
        write_skill(work_dir.path(), "git", "# Git Skill\n");

        let prompt =
            ContextManager::new(work_dir.path(), vec!["git".to_string(), "rust".to_string()])
                .build_system_prompt(false)
                .unwrap();

        let git_index = prompt.find("id: git").unwrap();
        let rust_index = prompt.find("id: rust").unwrap();
        assert!(git_index < rust_index);
        assert!(prompt.contains("# Available Skills"));
        assert!(prompt.contains("id: git"));
        assert!(prompt.contains("id: rust"));
        assert!(prompt.contains("name: Rust"));
        assert!(prompt.contains("description: Rust workflows"));
        assert!(!prompt.contains("# Git Skill"));
        assert!(!prompt.contains("# Rust Skill"));
    }

    #[test]
    fn hidden_active_skill_is_not_rendered_in_catalog() {
        let work_dir = tempdir().unwrap();
        write_skill(
            work_dir.path(),
            "secret",
            "---\nname: Secret\ndescription: Hidden workflow\ndisable-model-invocation: true\n---\n\n# Secret Skill\n",
        );

        let prompt = ContextManager::new(work_dir.path(), vec!["secret".to_string()])
            .build_system_prompt(false)
            .unwrap();

        assert!(!prompt.contains("# Available Skills"));
        assert!(!prompt.contains("secret"));
        assert!(!prompt.contains("Hidden workflow"));
        assert!(!prompt.contains("# Secret Skill"));
    }

    #[test]
    fn plan_mode_adds_externalized_state_instructions() {
        let work_dir = tempdir().unwrap();

        let prompt = ContextManager::new(work_dir.path(), Vec::new())
            .build_system_prompt(true)
            .unwrap();

        assert!(prompt.contains("# Plan Mode"));
        assert!(prompt.contains("PLAN.md"));
        assert!(prompt.contains("TODO.md"));
        assert!(prompt.contains("- [ ]"));
        assert!(prompt.contains("- [x]"));
    }

    fn write_skill(work_dir: &Path, skill_id: &str, content: &str) {
        let skill_dir = work_dir.join(".tiny-claw").join("skills").join(skill_id);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }
}
