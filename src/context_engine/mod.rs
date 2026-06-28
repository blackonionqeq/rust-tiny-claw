mod skills;

use skills::load_active_skills;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

const BASE_INSTRUCTIONS: &str =
    "You are rust-tiny-claw, a small coding assistant running inside one workspace.";

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

    pub fn build_system_prompt(&self) -> Result<String, ContextError> {
        let mut sections = vec![format!("# Base Instructions\n\n{BASE_INSTRUCTIONS}")];

        if let Some(workspace_instructions) = self.load_workspace_instructions()? {
            sections.push(format!(
                "# Workspace Instructions\n\nThe following instructions were loaded from AGENTS.md.\n\n{}",
                workspace_instructions.trim()
            ));
        }

        let skills = load_active_skills(&self.work_dir, &self.active_skills)?;
        if !skills.is_empty() {
            let mut rendered = String::from("# Active Skills");
            for skill in skills {
                rendered.push_str(&format!(
                    "\n\n## {}\n\nSource: {}\n\n{}",
                    skill.id,
                    skill.source.display(),
                    skill.body.trim()
                ));
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
    ReadFile { path: PathBuf, source: io::Error },
}

impl fmt::Display for ContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSkillId(skill_id) => write!(formatter, "invalid skill id: {skill_id}"),
            Self::ReadFile { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidSkillId(_) => None,
            Self::ReadFile { source, .. } => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn base_prompt_renders_without_workspace_or_skills() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();

        let prompt = ContextManager::new(&work_dir, Vec::new())
            .build_system_prompt()
            .unwrap();

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(prompt.contains("# Base Instructions"));
        assert!(prompt.contains(BASE_INSTRUCTIONS));
        assert!(!prompt.contains("# Workspace Instructions"));
        assert!(!prompt.contains("# Active Skills"));
    }

    #[test]
    fn agents_md_content_appears_in_workspace_section() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();
        fs::write(work_dir.join("AGENTS.md"), "Use project conventions.\n").unwrap();

        let prompt = ContextManager::new(&work_dir, Vec::new())
            .build_system_prompt()
            .unwrap();

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(prompt.contains("# Workspace Instructions"));
        assert!(prompt.contains("Use project conventions."));
    }

    #[test]
    fn active_skills_render_in_requested_order() {
        let work_dir = unique_temp_dir();
        write_skill(&work_dir, "rust", "# Rust Skill\n");
        write_skill(&work_dir, "git", "# Git Skill\n");

        let prompt = ContextManager::new(&work_dir, vec!["git".to_string(), "rust".to_string()])
            .build_system_prompt()
            .unwrap();

        fs::remove_dir_all(&work_dir).unwrap();

        let git_index = prompt.find("## git").unwrap();
        let rust_index = prompt.find("## rust").unwrap();
        assert!(git_index < rust_index);
        assert!(prompt.contains("# Git Skill"));
        assert!(prompt.contains("# Rust Skill"));
    }

    #[test]
    fn active_skill_frontmatter_is_not_rendered() {
        let work_dir = unique_temp_dir();
        write_skill(
            &work_dir,
            "rust",
            "---\nname: rust\ndescription: Rust conventions\n---\n\n# Rust Skill\n",
        );

        let prompt = ContextManager::new(&work_dir, vec!["rust".to_string()])
            .build_system_prompt()
            .unwrap();

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(prompt.contains("# Rust Skill"));
        assert!(!prompt.contains("description: Rust conventions"));
    }

    fn write_skill(work_dir: &Path, skill_id: &str, content: &str) {
        let skill_dir = work_dir.join(".tiny-claw").join("skills").join(skill_id);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    fn unique_temp_dir() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rust-tiny-claw-context-test-{suffix}"))
    }
}
