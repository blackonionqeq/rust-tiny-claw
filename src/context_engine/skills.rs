use super::ContextError;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillManifest {
    pub id: String,
    pub source: PathBuf,
    pub name: String,
    pub description: Option<String>,
    pub disable_model_invocation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDocument {
    pub id: String,
    pub source: PathBuf,
    pub body: String,
}

pub fn load_active_skill_manifests(
    work_dir: &Path,
    active_skills: &[String],
) -> Result<Vec<SkillManifest>, ContextError> {
    active_skills
        .iter()
        .map(|skill_id| load_skill_manifest(work_dir, skill_id))
        .collect()
}

#[cfg(test)]
pub fn load_active_skills(
    work_dir: &Path,
    active_skills: &[String],
) -> Result<Vec<SkillDocument>, ContextError> {
    active_skills
        .iter()
        .map(|skill_id| load_skill(work_dir, skill_id))
        .collect()
}

pub fn load_model_invokable_skill(
    work_dir: &Path,
    active_skills: &[String],
    skill_id: &str,
) -> Result<SkillDocument, ContextError> {
    validate_skill_id(skill_id)?;
    if !active_skills.iter().any(|active| active == skill_id) {
        return Err(ContextError::SkillNotEnabled(skill_id.to_string()));
    }

    let manifest = load_skill_manifest(work_dir, skill_id)?;
    if manifest.disable_model_invocation {
        return Err(ContextError::SkillModelInvocationDisabled(
            skill_id.to_string(),
        ));
    }

    load_skill(work_dir, skill_id)
}

fn load_skill_manifest(work_dir: &Path, skill_id: &str) -> Result<SkillManifest, ContextError> {
    let (source, path, content) = read_skill_file(work_dir, skill_id)?;
    let parsed = parse_simple_frontmatter(&path, &content)?;

    Ok(SkillManifest {
        id: skill_id.to_string(),
        source,
        name: parsed.name.unwrap_or_else(|| skill_id.to_string()),
        description: parsed.description,
        disable_model_invocation: parsed.disable_model_invocation.unwrap_or(false),
    })
}

fn load_skill(work_dir: &Path, skill_id: &str) -> Result<SkillDocument, ContextError> {
    let (source, path, content) = read_skill_file(work_dir, skill_id)?;
    let parsed = parse_simple_frontmatter(&path, &content)?;

    Ok(SkillDocument {
        id: skill_id.to_string(),
        source,
        body: parsed.body,
    })
}

fn read_skill_file(
    work_dir: &Path,
    skill_id: &str,
) -> Result<(PathBuf, PathBuf, String), ContextError> {
    validate_skill_id(skill_id)?;

    let source = PathBuf::from(".tiny-claw")
        .join("skills")
        .join(skill_id)
        .join("SKILL.md");
    let path = work_dir.join(&source);
    let content = fs::read_to_string(&path).map_err(|source| ContextError::ReadFile {
        path: path.clone(),
        source,
    })?;

    Ok((source, path, content))
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

#[derive(Debug, Default)]
struct ParsedSkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    disable_model_invocation: Option<bool>,
    body: String,
}

fn parse_simple_frontmatter(
    path: &Path,
    content: &str,
) -> Result<ParsedSkillFrontmatter, ContextError> {
    let rest_start = if content.starts_with("---\r\n") {
        "---\r\n".len()
    } else if content.starts_with("---\n") {
        "---\n".len()
    } else {
        return Ok(ParsedSkillFrontmatter {
            body: content.to_string(),
            ..ParsedSkillFrontmatter::default()
        });
    };

    let rest = &content[rest_start..];
    let Some((end, marker_len)) = find_frontmatter_end(rest) else {
        return Ok(ParsedSkillFrontmatter {
            body: content.to_string(),
            ..ParsedSkillFrontmatter::default()
        });
    };

    let frontmatter = &rest[..end];
    let Some(mut parsed) = parse_frontmatter_fields(path, frontmatter)? else {
        return Ok(ParsedSkillFrontmatter {
            body: content.to_string(),
            ..ParsedSkillFrontmatter::default()
        });
    };

    parsed.body = rest[end + marker_len..].to_string();
    Ok(parsed)
}

fn parse_frontmatter_fields(
    path: &Path,
    frontmatter: &str,
) -> Result<Option<ParsedSkillFrontmatter>, ContextError> {
    let mut parsed = ParsedSkillFrontmatter::default();

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Some((key, value)) = trimmed.split_once(':') else {
            return Ok(None);
        };

        let key = key.trim();
        let value = value.trim();
        if value.contains(':') {
            return Ok(None);
        }

        match key {
            "name" => parsed.name = Some(value.to_string()),
            "description" => parsed.description = Some(value.to_string()),
            "disable-model-invocation" => match value {
                "true" => parsed.disable_model_invocation = Some(true),
                "false" => parsed.disable_model_invocation = Some(false),
                _ => {
                    return Err(ContextError::InvalidSkillMetadata {
                        path: path.to_path_buf(),
                        message: "disable-model-invocation must be either true or false"
                            .to_string(),
                    });
                }
            },
            _ => return Ok(None),
        }
    }

    Ok(Some(parsed))
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
    use tempfile::tempdir;

    #[test]
    fn explicit_skills_load_in_requested_order() {
        let work_dir = tempdir().unwrap();
        write_skill(work_dir.path(), "rust", "# Rust Skill\n");
        write_skill(work_dir.path(), "git", "# Git Skill\n");

        let skills =
            load_active_skills(work_dir.path(), &["git".to_string(), "rust".to_string()]).unwrap();

        assert_eq!(skills[0].id, "git");
        assert_eq!(skills[0].body, "# Git Skill\n");
        assert_eq!(skills[1].id, "rust");
        assert_eq!(skills[1].body, "# Rust Skill\n");
    }

    #[test]
    fn active_skill_manifests_load_metadata() {
        let work_dir = tempdir().unwrap();
        write_skill(
            work_dir.path(),
            "rust",
            "---\nname: Rust\ndescription: Prefer Cargo workflows.\ndisable-model-invocation: false\n---\n\n# Rust Skill\n",
        );

        let manifests =
            load_active_skill_manifests(work_dir.path(), &["rust".to_string()]).unwrap();

        assert_eq!(manifests[0].id, "rust");
        assert_eq!(manifests[0].name, "Rust");
        assert_eq!(
            manifests[0].description.as_deref(),
            Some("Prefer Cargo workflows.")
        );
        assert!(!manifests[0].disable_model_invocation);
    }

    #[test]
    fn manifest_defaults_to_model_invokable() {
        let work_dir = tempdir().unwrap();
        write_skill(work_dir.path(), "rust", "# Rust Skill\n");

        let manifests =
            load_active_skill_manifests(work_dir.path(), &["rust".to_string()]).unwrap();

        assert_eq!(manifests[0].name, "rust");
        assert_eq!(manifests[0].description, None);
        assert!(!manifests[0].disable_model_invocation);
    }

    #[test]
    fn invalid_disable_model_invocation_returns_error() {
        let work_dir = tempdir().unwrap();
        write_skill(
            work_dir.path(),
            "rust",
            "---\ndisable-model-invocation: yes\n---\n\n# Rust Skill\n",
        );

        let error =
            load_active_skill_manifests(work_dir.path(), &["rust".to_string()]).unwrap_err();

        assert!(error.to_string().contains("disable-model-invocation"));
        assert!(error.to_string().contains("true or false"));
    }

    #[test]
    fn missing_explicit_skill_returns_error() {
        let work_dir = tempdir().unwrap();

        let error = load_active_skills(work_dir.path(), &["missing".to_string()]).unwrap_err();

        assert!(error.to_string().contains(".tiny-claw"));
        assert!(error.to_string().contains("missing"));
    }

    #[test]
    fn valid_frontmatter_is_removed_from_skill_body() {
        let work_dir = tempdir().unwrap();
        write_skill(
            work_dir.path(),
            "rust",
            "---\nname: rust\ndescription: Rust conventions\n---\n\n# Rust Skill\n",
        );

        let skills = load_active_skills(work_dir.path(), &["rust".to_string()]).unwrap();

        assert_eq!(skills[0].body, "\n# Rust Skill\n");
    }

    #[test]
    fn malformed_frontmatter_renders_full_skill_file() {
        let work_dir = tempdir().unwrap();
        write_skill(
            work_dir.path(),
            "rust",
            "---\ntags:\n- rust\n---\n\n# Rust Skill\n",
        );

        let skills = load_active_skills(work_dir.path(), &["rust".to_string()]).unwrap();

        assert!(skills[0].body.contains("tags:"));
        assert!(skills[0].body.contains("- rust"));
        assert!(skills[0].body.contains("# Rust Skill"));
    }

    #[test]
    fn model_invokable_skill_rejects_hidden_skill() {
        let work_dir = tempdir().unwrap();
        write_skill(
            work_dir.path(),
            "secret",
            "---\ndisable-model-invocation: true\n---\n\n# Secret Skill\n",
        );

        let error = load_model_invokable_skill(work_dir.path(), &["secret".to_string()], "secret")
            .unwrap_err();

        assert!(error.to_string().contains("disabled for model invocation"));
    }

    #[test]
    fn model_invokable_skill_rejects_unenabled_skill() {
        let work_dir = tempdir().unwrap();
        write_skill(work_dir.path(), "rust", "# Rust Skill\n");

        let error = load_model_invokable_skill(work_dir.path(), &[], "rust").unwrap_err();

        assert!(error.to_string().contains("not enabled"));
    }

    #[test]
    fn crlf_frontmatter_is_removed_from_skill_body() {
        let work_dir = tempdir().unwrap();
        write_skill(
            work_dir.path(),
            "rust",
            "---\r\nname: rust\r\ndescription: Rust conventions\r\n---\r\n\r\n# Rust Skill\r\n",
        );

        let skills = load_active_skills(work_dir.path(), &["rust".to_string()]).unwrap();

        assert_eq!(skills[0].body, "\r\n# Rust Skill\r\n");
    }

    fn write_skill(work_dir: &Path, skill_id: &str, content: &str) {
        let skill_dir = work_dir.join(".tiny-claw").join("skills").join(skill_id);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }
}
