use super::ContextError;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDocument {
    pub id: String,
    pub source: PathBuf,
    pub body: String,
}

pub fn load_active_skills(
    work_dir: &Path,
    active_skills: &[String],
) -> Result<Vec<SkillDocument>, ContextError> {
    active_skills
        .iter()
        .map(|skill_id| load_skill(work_dir, skill_id))
        .collect()
}

fn load_skill(work_dir: &Path, skill_id: &str) -> Result<SkillDocument, ContextError> {
    validate_skill_id(skill_id)?;

    let source = PathBuf::from(".tiny-claw")
        .join("skills")
        .join(skill_id)
        .join("SKILL.md");
    let path = work_dir.join(&source);
    let content =
        fs::read_to_string(&path).map_err(|source| ContextError::ReadFile { path, source })?;
    let body = strip_simple_frontmatter(&content);

    Ok(SkillDocument {
        id: skill_id.to_string(),
        source,
        body,
    })
}

fn validate_skill_id(skill_id: &str) -> Result<(), ContextError> {
    if skill_id.is_empty()
        || Path::new(skill_id).is_absolute()
        || Path::new(skill_id)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ContextError::InvalidSkillId(skill_id.to_string()));
    }

    Ok(())
}

fn strip_simple_frontmatter(content: &str) -> String {
    let rest_start = if content.starts_with("---\r\n") {
        "---\r\n".len()
    } else if content.starts_with("---\n") {
        "---\n".len()
    } else {
        return content.to_string();
    };

    let rest = &content[rest_start..];
    let Some((end, marker_len)) = find_frontmatter_end(rest) else {
        return content.to_string();
    };

    let frontmatter = &rest[..end];
    if !is_simple_frontmatter(frontmatter) {
        return content.to_string();
    }

    rest[end + marker_len..].to_string()
}

fn is_simple_frontmatter(frontmatter: &str) -> bool {
    frontmatter.lines().all(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return true;
        }

        let Some((key, value)) = trimmed.split_once(':') else {
            return false;
        };

        matches!(key.trim(), "name" | "description") && !value.trim().contains(':')
    })
}

fn find_frontmatter_end(rest: &str) -> Option<(usize, usize)> {
    ["\n---\n", "\n---\r\n"]
        .into_iter()
        .filter_map(|marker| rest.find(marker).map(|index| (index, marker.len())))
        .min_by_key(|(index, _)| *index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn explicit_skills_load_in_requested_order() {
        let work_dir = unique_temp_dir();
        write_skill(&work_dir, "rust", "# Rust Skill\n");
        write_skill(&work_dir, "git", "# Git Skill\n");

        let skills =
            load_active_skills(&work_dir, &["git".to_string(), "rust".to_string()]).unwrap();

        fs::remove_dir_all(&work_dir).unwrap();

        assert_eq!(skills[0].id, "git");
        assert_eq!(skills[0].body, "# Git Skill\n");
        assert_eq!(skills[1].id, "rust");
        assert_eq!(skills[1].body, "# Rust Skill\n");
    }

    #[test]
    fn missing_explicit_skill_returns_error() {
        let work_dir = unique_temp_dir();
        fs::create_dir_all(&work_dir).unwrap();

        let error = load_active_skills(&work_dir, &["missing".to_string()]).unwrap_err();

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(error.to_string().contains(".tiny-claw"));
        assert!(error.to_string().contains("missing"));
    }

    #[test]
    fn valid_frontmatter_is_removed_from_skill_body() {
        let work_dir = unique_temp_dir();
        write_skill(
            &work_dir,
            "rust",
            "---\nname: rust\ndescription: Rust conventions\n---\n\n# Rust Skill\n",
        );

        let skills = load_active_skills(&work_dir, &["rust".to_string()]).unwrap();

        fs::remove_dir_all(&work_dir).unwrap();

        assert_eq!(skills[0].body, "\n# Rust Skill\n");
    }

    #[test]
    fn malformed_frontmatter_renders_full_skill_file() {
        let work_dir = unique_temp_dir();
        write_skill(
            &work_dir,
            "rust",
            "---\ntags:\n- rust\n---\n\n# Rust Skill\n",
        );

        let skills = load_active_skills(&work_dir, &["rust".to_string()]).unwrap();

        fs::remove_dir_all(&work_dir).unwrap();

        assert!(skills[0].body.contains("tags:"));
        assert!(skills[0].body.contains("- rust"));
        assert!(skills[0].body.contains("# Rust Skill"));
    }

    #[test]
    fn crlf_frontmatter_is_removed_from_skill_body() {
        let work_dir = unique_temp_dir();
        write_skill(
            &work_dir,
            "rust",
            "---\r\nname: rust\r\ndescription: Rust conventions\r\n---\r\n\r\n# Rust Skill\r\n",
        );

        let skills = load_active_skills(&work_dir, &["rust".to_string()]).unwrap();

        fs::remove_dir_all(&work_dir).unwrap();

        assert_eq!(skills[0].body, "\r\n# Rust Skill\r\n");
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
        std::env::temp_dir().join(format!("rust-tiny-claw-skills-test-{suffix}"))
    }
}
