use crate::context_engine::load_model_invokable_skill;
use crate::schema::{ToolCall, ToolResult};
use crate::tools::{Tool, ToolAccessMode};
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug)]
pub struct LoadSkillTool {
    work_dir: PathBuf,
    active_skills: Vec<String>,
    loaded_skills: Mutex<HashSet<String>>,
}

impl LoadSkillTool {
    pub fn new(
        work_dir: impl Into<PathBuf>,
        active_skills: Vec<String>,
    ) -> Result<Self, std::io::Error> {
        let work_dir = work_dir.into().canonicalize()?;
        Ok(Self {
            work_dir,
            active_skills,
            loaded_skills: Mutex::new(HashSet::new()),
        })
    }
}

impl Tool for LoadSkillTool {
    fn name(&self) -> &'static str {
        "load_skill"
    }

    fn description(&self) -> &'static str {
        "Load the full instructions for an enabled skill from the Available Skills catalog."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "skill_id": {
                    "type": "string",
                    "description": "Enabled skill id to load, such as rust."
                }
            },
            "required": ["skill_id"]
        })
    }

    fn access_mode(&self, _call: &ToolCall) -> ToolAccessMode {
        ToolAccessMode::ReadOnly
    }

    fn execute(&self, call: &ToolCall) -> ToolResult {
        let Some(skill_id) = call
            .arguments
            .get("skill_id")
            .and_then(|value| value.as_str())
        else {
            return ToolResult::error(call.id.clone(), "missing string argument: skill_id");
        };

        let mut loaded_skills = self.loaded_skills.lock().unwrap();
        if loaded_skills.contains(skill_id) {
            return ToolResult::ok(
                call.id.clone(),
                format!("skill '{skill_id}' is already loaded"),
            );
        }

        let skill = match load_model_invokable_skill(&self.work_dir, &self.active_skills, skill_id)
        {
            Ok(skill) => skill,
            Err(error) => return ToolResult::error(call.id.clone(), error.to_string()),
        };

        loaded_skills.insert(skill_id.to_string());

        ToolResult::ok(
            call.id.clone(),
            format!(
                "skill: {}\nsource: {}\n\n{}",
                skill.id,
                skill.source.display(),
                skill.body.trim()
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::LoadSkillTool;
    use crate::schema::ToolCall;
    use crate::tools::Tool;
    use serde_json::json;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn load_skill_returns_enabled_model_invokable_body() {
        let work_dir = tempdir().unwrap();
        write_skill(
            work_dir.path(),
            "rust",
            "---\nname: Rust\ndescription: Prefer Cargo.\n---\n\n# Rust Skill\nPrefer cargo.\n",
        );

        let tool = LoadSkillTool::new(work_dir.path(), vec!["rust".to_string()]).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "load_skill",
            json!({ "skill_id": "rust" }),
        ));

        assert!(!result.is_error);
        assert!(result.output.contains("skill: rust"));
        assert!(result.output.contains("# Rust Skill"));
        assert!(!result.output.contains("description: Prefer Cargo."));
    }

    #[test]
    fn load_skill_rejects_hidden_skill() {
        let work_dir = tempdir().unwrap();
        write_skill(
            work_dir.path(),
            "secret",
            "---\ndisable-model-invocation: true\n---\n\n# Secret Skill\n",
        );

        let tool = LoadSkillTool::new(work_dir.path(), vec!["secret".to_string()]).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "load_skill",
            json!({ "skill_id": "secret" }),
        ));

        assert!(result.is_error);
        assert!(result.output.contains("disabled for model invocation"));
    }

    #[test]
    fn load_skill_rejects_unenabled_skill() {
        let work_dir = tempdir().unwrap();
        write_skill(work_dir.path(), "rust", "# Rust Skill\n");

        let tool = LoadSkillTool::new(work_dir.path(), Vec::new()).unwrap();
        let result = tool.execute(&ToolCall::new(
            "call_1",
            "load_skill",
            json!({ "skill_id": "rust" }),
        ));

        assert!(result.is_error);
        assert!(result.output.contains("not enabled"));
    }

    #[test]
    fn load_skill_is_idempotent() {
        let work_dir = tempdir().unwrap();
        write_skill(work_dir.path(), "rust", "# Rust Skill\n");

        let tool = LoadSkillTool::new(work_dir.path(), vec!["rust".to_string()]).unwrap();
        let first = tool.execute(&ToolCall::new(
            "call_1",
            "load_skill",
            json!({ "skill_id": "rust" }),
        ));
        let second = tool.execute(&ToolCall::new(
            "call_2",
            "load_skill",
            json!({ "skill_id": "rust" }),
        ));

        assert!(!first.is_error);
        assert!(!second.is_error);
        assert!(first.output.contains("# Rust Skill"));
        assert!(!second.output.contains("# Rust Skill"));
        assert!(second.output.contains("already loaded"));
    }

    fn write_skill(work_dir: &Path, skill_id: &str, content: &str) {
        let skill_dir = work_dir.join(".tiny-claw").join("skills").join(skill_id);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }
}
