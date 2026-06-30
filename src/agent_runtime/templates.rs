use crate::context_engine::ContextBudget;
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOverrides {
    pub role_prompt_append: Option<String>,
    pub extra_skills: Vec<String>,
    pub tool_profile: Option<String>,
    pub output_contract: Option<String>,
    pub context_budget: Option<ContextBudget>,
}

impl AgentOverrides {
    pub fn empty() -> Self {
        Self {
            role_prompt_append: None,
            extra_skills: Vec::new(),
            tool_profile: None,
            output_contract: None,
            context_budget: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    pub template_id: Option<String>,
    pub role: String,
    pub task: String,
    pub system_prompt: String,
    pub tool_profile: String,
    pub skills: Vec<String>,
    pub context_budget: ContextBudget,
    pub output_contract: String,
    pub parent_session_id: Option<String>,
}

#[derive(Debug, Clone)]
struct SubagentTemplate {
    id: String,
    name: String,
    description: String,
    role_prompt: String,
    tool_profile: String,
    skill_set: String,
    output_contract: String,
    allow_role_prompt_append: bool,
}

#[derive(Debug, Clone)]
pub struct SubagentTemplateRegistry {
    templates: HashMap<String, SubagentTemplate>,
    skill_sets: HashMap<String, Vec<String>>,
}

impl SubagentTemplateRegistry {
    pub fn built_in() -> Self {
        let mut templates = HashMap::new();
        templates.insert(
            "explorer".to_string(),
            SubagentTemplate {
                id: "explorer".to_string(),
                name: "Explorer Subagent".to_string(),
                description: "Read-only repository exploration with evidence-backed summary."
                    .to_string(),
                role_prompt: "You are an explorer subagent. Investigate the requested topic using the available read-only tools. Return a concise report with concrete evidence, including file paths and relevant symbols when possible. Do not edit files or make final project decisions."
                    .to_string(),
                tool_profile: "read_only".to_string(),
                skill_set: "rust_explorer".to_string(),
                output_contract: "exploration_report".to_string(),
                allow_role_prompt_append: true,
            },
        );

        let mut skill_sets = HashMap::new();
        skill_sets.insert("rust_explorer".to_string(), vec!["subagents".to_string()]);

        Self {
            templates,
            skill_sets,
        }
    }

    pub fn resolve(
        &self,
        template_id: &str,
        task: String,
        overrides: AgentOverrides,
        parent_session_id: Option<String>,
    ) -> Result<AgentSpec, TemplateError> {
        let template =
            self.templates
                .get(template_id)
                .ok_or_else(|| TemplateError::UnknownTemplate {
                    id: template_id.to_string(),
                })?;

        // Model-provided overrides may narrow behavior, but the template owns
        // capabilities. Do not let a delegate call grant itself tools, skills,
        // output modes, or a larger context budget.
        if !overrides.extra_skills.is_empty()
            || overrides.tool_profile.is_some()
            || overrides.output_contract.is_some()
            || overrides.context_budget.is_some()
        {
            return Err(TemplateError::OverrideNotAllowed {
                template_id: template.id.clone(),
            });
        }

        let mut system_prompt = template.role_prompt.clone();
        if let Some(append) = overrides.role_prompt_append {
            if !template.allow_role_prompt_append {
                return Err(TemplateError::OverrideNotAllowed {
                    template_id: template.id.clone(),
                });
            }
            system_prompt.push_str("\n\n");
            system_prompt.push_str(append.trim());
        }

        let skills = self
            .skill_sets
            .get(&template.skill_set)
            .cloned()
            .ok_or_else(|| TemplateError::UnknownSkillSet {
                id: template.skill_set.clone(),
            })?;

        Ok(AgentSpec {
            template_id: Some(template.id.clone()),
            role: template.name.clone(),
            task,
            system_prompt,
            tool_profile: template.tool_profile.clone(),
            skills,
            context_budget: ContextBudget::default(),
            output_contract: template.output_contract.clone(),
            parent_session_id,
        })
    }

    pub fn describe_templates(&self) -> String {
        let mut templates = self.templates.values().collect::<Vec<_>>();
        templates.sort_by_key(|template| template.id.as_str());
        templates
            .into_iter()
            .map(|template| {
                format!(
                    "- {}: {} ({})",
                    template.id, template.name, template.description
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone)]
pub struct ToolProfileRegistry {
    profiles: HashMap<String, Vec<&'static str>>,
}

impl ToolProfileRegistry {
    pub fn built_in() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert(
            "read_only".to_string(),
            vec!["grep", "load_skill", "read_file"],
        );
        Self { profiles }
    }

    pub fn tools_for(&self, profile_id: &str) -> Result<Vec<&'static str>, TemplateError> {
        self.profiles
            .get(profile_id)
            .cloned()
            .ok_or_else(|| TemplateError::UnknownToolProfile {
                id: profile_id.to_string(),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    UnknownTemplate { id: String },
    UnknownToolProfile { id: String },
    UnknownSkillSet { id: String },
    OverrideNotAllowed { template_id: String },
}

impl fmt::Display for TemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTemplate { id } => write!(formatter, "unknown subagent template: {id}"),
            Self::UnknownToolProfile { id } => write!(formatter, "unknown tool profile: {id}"),
            Self::UnknownSkillSet { id } => write!(formatter, "unknown skill set: {id}"),
            Self::OverrideNotAllowed { template_id } => {
                write!(
                    formatter,
                    "override is not allowed for template '{template_id}'"
                )
            }
        }
    }
}

impl std::error::Error for TemplateError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_resolves_to_read_only_agent_spec() {
        let registry = SubagentTemplateRegistry::built_in();

        let spec = registry
            .resolve(
                "explorer",
                "inspect engine".to_string(),
                AgentOverrides::empty(),
                Some("parent".to_string()),
            )
            .unwrap();

        assert_eq!(spec.template_id.as_deref(), Some("explorer"));
        assert_eq!(spec.tool_profile, "read_only");
        assert_eq!(spec.skills, vec!["subagents"]);
        assert_eq!(spec.parent_session_id.as_deref(), Some("parent"));
        assert!(spec.system_prompt.contains("explorer subagent"));
    }

    #[test]
    fn capability_expanding_overrides_are_rejected() {
        let registry = SubagentTemplateRegistry::built_in();
        let mut overrides = AgentOverrides::empty();
        overrides.tool_profile = Some("tester".to_string());

        let error = registry
            .resolve("explorer", "inspect".to_string(), overrides, None)
            .unwrap_err();

        assert!(matches!(error, TemplateError::OverrideNotAllowed { .. }));
    }

    #[test]
    fn read_only_profile_excludes_bash() {
        let registry = ToolProfileRegistry::built_in();

        let tools = registry.tools_for("read_only").unwrap();

        assert_eq!(tools, vec!["grep", "load_skill", "read_file"]);
        assert!(!tools.contains(&"bash"));
    }
}
