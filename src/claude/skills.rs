use anyhow::{Context, Result};
use std::path::Path;

use crate::config::Paths;
use crate::persona::SkillsConfig;
use crate::symlink::replace_with_symlink;

/// Switch the skills symlink to point to the given persona's skill-set.
/// If ~/.claude/skills is a real directory (first-time use), auto-migrate it
/// into the target persona's skill-set.
pub fn switch_skills_symlink(paths: &Paths, persona_name: &str) -> Result<()> {
    let target = paths.skill_sets.join(persona_name);
    if !target.exists() {
        std::fs::create_dir_all(&target)?;
    }

    // If claude_skills is a real directory, migrate its contents first
    if paths.claude_skills.exists()
        && !is_symlink(&paths.claude_skills)
        && paths.claude_skills.is_dir()
    {
        eprintln!(
            "  Migrating existing skills directory into persona '{}'...",
            persona_name
        );
        migrate_dir_contents(&paths.claude_skills, &target)?;
        std::fs::remove_dir_all(&paths.claude_skills)
            .context("Failed to remove original skills directory after migration")?;
    }

    replace_with_symlink(&paths.claude_skills, &target)
        .context("Failed to switch skills symlink")?;
    Ok(())
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn migrate_dir_contents(src: &Path, dst: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if dst_path.exists() {
            continue; // Don't overwrite existing files in target
        }

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Apply per-skill enable/disable within the current skill-set directory.
/// Skills in `active` list get disable-model-invocation removed;
/// all others get it set to true.
pub fn apply_skill_toggles(skills_dir: &Path, config: &SkillsConfig) -> Result<()> {
    if !skills_dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(skills_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let skill_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }

        let is_active = config.active.iter().any(|a| a == &skill_name);
        set_skill_disabled(&skill_md, !is_active)?;
    }

    Ok(())
}

/// Toggle a single skill's disable-model-invocation.
pub fn toggle_skill(skill_md: &Path) -> Result<bool> {
    let content = std::fs::read_to_string(skill_md)?;
    let currently_disabled = is_disabled(&content);
    set_skill_disabled(skill_md, !currently_disabled)?;
    Ok(!currently_disabled)
}

/// List skills in a directory with their disabled status.
pub fn list_skills(skills_dir: &Path) -> Result<Vec<(String, bool)>> {
    let mut result = Vec::new();
    if !skills_dir.exists() {
        return Ok(result);
    }

    for entry in std::fs::read_dir(skills_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let skill_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }

        let content = std::fs::read_to_string(&skill_md)?;
        let disabled = is_disabled(&content);
        result.push((skill_name, disabled));
    }

    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

/// Remove a skill directory entirely.
pub fn remove_skill(skills_dir: &Path, name: &str) -> Result<()> {
    let skill_dir = skills_dir.join(name);
    if skill_dir.exists() {
        std::fs::remove_dir_all(&skill_dir)
            .with_context(|| format!("Failed to remove skill: {}", name))?;
    }
    Ok(())
}

/// Check if a SKILL.md has disable-model-invocation: true in frontmatter.
fn is_disabled(content: &str) -> bool {
    // Parse YAML frontmatter between --- markers
    if let Some(fm) = extract_frontmatter(content) {
        fm.contains("disable-model-invocation: true")
            || fm.contains("disable-model-invocation:true")
    } else {
        false
    }
}

/// Set or remove disable-model-invocation in SKILL.md frontmatter.
fn set_skill_disabled(path: &Path, disabled: bool) -> Result<()> {
    let content = std::fs::read_to_string(path)?;

    let new_content = if disabled {
        ensure_frontmatter_field(&content, "disable-model-invocation", "true")
    } else {
        remove_frontmatter_field(&content, "disable-model-invocation")
    };

    std::fs::write(path, new_content)?;
    Ok(())
}

fn extract_frontmatter(content: &str) -> Option<&str> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    if let Some(end) = rest.find("\n---") {
        Some(&rest[..end])
    } else {
        None
    }
}

fn ensure_frontmatter_field(content: &str, key: &str, value: &str) -> String {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        // No frontmatter — add one
        return format!("---\n{}: {}\n---\n{}", key, value, content);
    }

    let rest = &trimmed[3..];
    if let Some(end) = rest.find("\n---") {
        let fm = &rest[..end];
        let after = &rest[end + 4..]; // skip \n---

        // Check if field already exists
        let field_prefix = format!("{}:", key);
        let new_fm: String = if fm
            .lines()
            .any(|l| l.trim_start().starts_with(&field_prefix))
        {
            // Replace existing
            fm.lines()
                .map(|l| {
                    if l.trim_start().starts_with(&field_prefix) {
                        format!("{}: {}", key, value)
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            // Add new field
            format!("{}\n{}: {}", fm, key, value)
        };

        format!("---{}\n---{}", new_fm, after)
    } else {
        content.to_string()
    }
}

fn remove_frontmatter_field(content: &str, key: &str) -> String {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content.to_string();
    }

    let rest = &trimmed[3..];
    if let Some(end) = rest.find("\n---") {
        let fm = &rest[..end];
        let after = &rest[end + 4..];

        let field_prefix = format!("{}:", key);
        let new_fm: String = fm
            .lines()
            .filter(|l| !l.trim_start().starts_with(&field_prefix))
            .collect::<Vec<_>>()
            .join("\n");

        format!("---{}\n---{}", new_fm, after)
    } else {
        content.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;

    #[test]
    fn is_disabled_only_checks_frontmatter() {
        let frontmatter = "---\nname: demo\ndisable-model-invocation: true\n---\nBody\n";
        let body_only = "# Title\n\ndisable-model-invocation: true\n";

        assert!(is_disabled(frontmatter));
        assert!(!is_disabled(body_only));
    }

    #[test]
    fn ensure_frontmatter_field_handles_missing_existing_and_replacement() {
        let no_frontmatter = "Body\n";
        let added = ensure_frontmatter_field(no_frontmatter, "disable-model-invocation", "true");
        assert!(added.starts_with("---\ndisable-model-invocation: true\n---\n"));
        assert!(added.ends_with("Body\n"));

        let with_frontmatter = "---\nname: demo\n---\nBody\n";
        let inserted =
            ensure_frontmatter_field(with_frontmatter, "disable-model-invocation", "true");
        assert!(inserted.contains("name: demo"));
        assert!(inserted.contains("disable-model-invocation: true"));
        assert!(inserted.ends_with("\nBody\n"));

        let with_existing = "---\nname: demo\ndisable-model-invocation: false\n---\nBody\n";
        let replaced = ensure_frontmatter_field(with_existing, "disable-model-invocation", "true");
        assert!(replaced.contains("disable-model-invocation: true"));
        assert!(!replaced.contains("disable-model-invocation: false"));
        assert!(replaced.contains("name: demo"));
    }

    #[test]
    fn remove_frontmatter_field_preserves_other_fields_and_body() {
        let content =
            "---\nname: demo\ndescription: test\ndisable-model-invocation: true\n---\nBody\n";
        let updated = remove_frontmatter_field(content, "disable-model-invocation");

        assert!(updated.contains("name: demo"));
        assert!(updated.contains("description: test"));
        assert!(!updated.contains("disable-model-invocation"));
        assert!(updated.ends_with("\nBody\n"));
    }

    #[test]
    fn toggle_skill_round_trips_disable_flag() {
        let env = TestEnv::new();
        let skill_md = env.paths.root.join("demo").join("SKILL.md");
        env.write_file(&skill_md, "---\nname: demo\n---\nBody\n");

        let disabled = toggle_skill(&skill_md).unwrap();
        assert!(disabled);
        assert!(is_disabled(&env.read_file(&skill_md)));

        let enabled = toggle_skill(&skill_md).unwrap();
        assert!(!enabled);
        assert!(!is_disabled(&env.read_file(&skill_md)));
    }

    #[test]
    fn apply_skill_toggles_updates_each_skill_based_on_active_list() {
        let env = TestEnv::new();
        let skills_dir = env.paths.root.join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        env.create_skill(&skills_dir, "alpha", "---\nname: alpha\n---\n");
        env.create_skill(
            &skills_dir,
            "beta",
            "---\nname: beta\ndisable-model-invocation: true\n---\n",
        );

        apply_skill_toggles(
            &skills_dir,
            &SkillsConfig {
                active: vec!["alpha".to_string()],
            },
        )
        .unwrap();

        assert!(!is_disabled(
            &env.read_file(&skills_dir.join("alpha").join("SKILL.md"))
        ));
        assert!(is_disabled(
            &env.read_file(&skills_dir.join("beta").join("SKILL.md"))
        ));
    }
}
