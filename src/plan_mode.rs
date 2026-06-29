use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanModeSetting {
    Auto,
    On,
}

impl PlanModeSetting {
    pub fn resolve(self, prompt: &str, work_dir: &Path) -> bool {
        match self {
            Self::On => true,
            Self::Auto => should_enable_plan_mode(prompt, work_dir),
        }
    }
}

pub fn should_enable_plan_mode(prompt: &str, work_dir: &Path) -> bool {
    if work_dir.join("PLAN.md").is_file() || work_dir.join("TODO.md").is_file() {
        return true;
    }

    let normalized = prompt.to_lowercase();
    let complex_markers = [
        "refactor",
        "implement",
        "migrate",
        "tests",
        "continue",
        "plan",
        "todo",
        "step by step",
        "multi-file",
        "project",
        "architecture",
        "重构",
        "实现",
        "迁移",
        "测试",
        "继续",
        "计划",
        "待办",
        "分步骤",
        "项目",
        "架构",
        "多个文件",
    ];
    let connector_markers = [" and ", " then ", " also ", "并且", "同时", "然后", "以及"];

    if normalized.chars().count() >= 120 {
        return true;
    }

    let complex_hits = complex_markers
        .iter()
        .filter(|marker| normalized.contains(**marker))
        .count();
    let connector_hits = connector_markers
        .iter()
        .filter(|marker| normalized.contains(**marker))
        .count();

    complex_hits >= 2 || (complex_hits >= 1 && connector_hits >= 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn on_setting_always_enables_plan_mode() {
        assert!(PlanModeSetting::On.resolve("List files", &PathBuf::from("/missing")));
    }

    #[test]
    fn auto_setting_enables_for_existing_plan_files() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();
        fs::write(work_dir.join("TODO.md"), "- [ ] Continue\n").unwrap();

        let enabled = PlanModeSetting::Auto.resolve("List files", &work_dir);

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(enabled);
    }

    #[test]
    fn auto_setting_enables_for_complex_prompt() {
        assert!(PlanModeSetting::Auto.resolve(
            "Refactor the project architecture and add tests",
            &PathBuf::from("/missing"),
        ));
        assert!(
            PlanModeSetting::Auto
                .resolve("继续实现这个项目，并且补充测试", &PathBuf::from("/missing"),)
        );
    }

    #[test]
    fn auto_setting_stays_light_for_simple_prompt() {
        assert!(!PlanModeSetting::Auto.resolve("List files", &PathBuf::from("/missing"),));
        assert!(!PlanModeSetting::Auto.resolve("解释这个函数", &PathBuf::from("/missing"),));
    }

    fn unique_temp_dir() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rust-tiny-claw-plan-mode-test-{suffix}"))
    }
}
