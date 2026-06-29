use crate::schema::ToolCall;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Ask { reason: String },
    Deny { reason: String },
}

impl PermissionDecision {
    fn from_rule(decision: RuleDecision, reason: &str) -> Self {
        match decision {
            RuleDecision::Allow => Self::Allow,
            RuleDecision::Ask => Self::Ask {
                reason: reason.to_string(),
            },
            RuleDecision::Deny => Self::Deny {
                reason: reason.to_string(),
            },
        }
    }
}

pub trait ToolPolicy: Send + Sync {
    fn decide(&self, call: &ToolCall) -> PermissionDecision;
}

#[derive(Debug, Clone)]
pub struct RuleBasedToolPolicy {
    rules: Vec<ToolRule>,
}

impl RuleBasedToolPolicy {
    pub fn new(rules: Vec<ToolRule>) -> Self {
        Self { rules }
    }

    pub fn feishu_default() -> Self {
        Self::new(vec![
            ToolRule::new(
                "bash",
                "command",
                TextPattern::All(vec!["rm".to_string(), "-r".to_string()]),
                RuleDecision::Ask,
                "recursive deletion requires human approval",
            ),
            ToolRule::new(
                "bash",
                "command",
                TextPattern::All(vec!["rm".to_string(), "-fr".to_string()]),
                RuleDecision::Ask,
                "recursive deletion requires human approval",
            ),
            ToolRule::new(
                "bash",
                "command",
                TextPattern::Contains("sudo ".to_string()),
                RuleDecision::Ask,
                "privileged command execution requires human approval",
            ),
            ToolRule::new(
                "bash",
                "command",
                TextPattern::Any(vec![
                    "drop table".to_string(),
                    "drop database".to_string(),
                    "truncate table".to_string(),
                ]),
                RuleDecision::Ask,
                "destructive database command requires human approval",
            ),
            ToolRule::new(
                "bash",
                "command",
                TextPattern::All(vec!["kubectl".to_string(), "delete".to_string()]),
                RuleDecision::Ask,
                "cluster deletion command requires human approval",
            ),
        ])
    }
}

impl Default for RuleBasedToolPolicy {
    fn default() -> Self {
        Self::feishu_default()
    }
}

impl ToolPolicy for RuleBasedToolPolicy {
    fn decide(&self, call: &ToolCall) -> PermissionDecision {
        self.rules
            .iter()
            .find(|rule| rule.matches(call))
            .map(|rule| PermissionDecision::from_rule(rule.decision, &rule.reason))
            .unwrap_or(PermissionDecision::Allow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRule {
    tool: String,
    argument: String,
    pattern: TextPattern,
    decision: RuleDecision,
    reason: String,
}

impl ToolRule {
    pub fn new(
        tool: impl Into<String>,
        argument: impl Into<String>,
        pattern: TextPattern,
        decision: RuleDecision,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            tool: tool.into(),
            argument: argument.into(),
            pattern,
            decision,
            reason: reason.into(),
        }
    }

    fn matches(&self, call: &ToolCall) -> bool {
        if call.name != self.tool {
            return false;
        }

        let Some(value) = string_argument(&call.arguments, &self.argument) else {
            return false;
        };

        self.pattern.matches(value)
    }
}

fn string_argument<'a>(arguments: &'a Value, name: &str) -> Option<&'a str> {
    arguments.get(name).and_then(Value::as_str)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextPattern {
    Contains(String),
    Any(Vec<String>),
    All(Vec<String>),
}

impl TextPattern {
    fn matches(&self, value: &str) -> bool {
        let normalized = value.to_ascii_lowercase();
        match self {
            Self::Contains(pattern) => normalized.contains(&pattern.to_ascii_lowercase()),
            Self::Any(patterns) => patterns
                .iter()
                .any(|pattern| normalized.contains(&pattern.to_ascii_lowercase())),
            Self::All(patterns) => patterns
                .iter()
                .all(|pattern| normalized.contains(&pattern.to_ascii_lowercase())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PermissionDecision, RuleBasedToolPolicy, ToolPolicy};
    use crate::schema::ToolCall;
    use serde_json::json;

    #[test]
    fn default_policy_allows_safe_bash_commands() {
        let policy = RuleBasedToolPolicy::default();
        let call = ToolCall::new("call_1", "bash", json!({ "command": "git status" }));

        assert_eq!(policy.decide(&call), PermissionDecision::Allow);
    }

    #[test]
    fn default_policy_asks_for_recursive_force_delete() {
        let policy = RuleBasedToolPolicy::default();
        let call = ToolCall::new("call_1", "bash", json!({ "command": "rm -rf target" }));

        assert!(matches!(
            policy.decide(&call),
            PermissionDecision::Ask { reason } if reason.contains("recursive deletion")
        ));
    }

    #[test]
    fn default_policy_asks_for_recursive_delete_without_force() {
        let policy = RuleBasedToolPolicy::default();
        let call = ToolCall::new("call_1", "bash", json!({ "command": "rm -r target" }));

        assert!(matches!(
            policy.decide(&call),
            PermissionDecision::Ask { reason } if reason.contains("recursive deletion")
        ));
    }

    #[test]
    fn default_policy_asks_for_cluster_deletion() {
        let policy = RuleBasedToolPolicy::default();
        let call = ToolCall::new(
            "call_1",
            "bash",
            json!({ "command": "kubectl delete pod api-1" }),
        );

        assert!(matches!(
            policy.decide(&call),
            PermissionDecision::Ask { reason } if reason.contains("cluster deletion")
        ));
    }
}
