use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::config::Paths;
use crate::persona::{Persona, SkillsConfig};
use crate::symlink::{self, is_symlink, replace_with_symlink};

/// The cc-persona skill is always managed and always linked, regardless of TOML.
const CC_PERSONA_SKILL: &str = "cc-persona";

/// Outcome of reconciling `~/.claude/skills` against a persona's desired skill set.
///
/// Each vector holds skill names. The classification follows the filesystem-level
/// invariants (I2/I3): managed skills are symlinks into the store, untracked skills
/// are real subdirectories that we never touch.
#[derive(Debug, Default)]
pub struct ReconcileReport {
    /// Skills newly linked into `~/.claude/skills` during this run.
    pub linked: Vec<String>,
    /// Managed links removed because the skill is no longer desired.
    pub unlinked: Vec<String>,
    /// Real subdirectories left in place (wild / not managed by cc-persona).
    pub untracked: Vec<String>,
    /// Desired skills with no directory in the store (cannot be linked).
    pub ghosts: Vec<String>,
    /// Desired skills blocked by a same-named wild directory already present.
    pub shadowed: Vec<String>,
    /// Symlinks pointing outside the skill-store (foreign / externally managed).
    pub foreign_links: Vec<String>,
}

/// A skill entry in `~/.claude/skills` annotated with whether it is managed
/// (a symlink into the store) and whether model invocation is disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: String,
    pub disabled: bool,
    pub managed: bool,
}

/// Reconcile `~/.claude/skills` so it exactly reflects the persona's desired skill set.
///
/// Desired = `persona.skills.active` ∪ `{cc-persona}`. The store is the single source
/// of truth for skill contents; this only manages the per-skill symlinks under
/// `~/.claude/skills` (a real directory, invariant I1).
///
/// Behaviour:
/// - Existing symlink whose target escapes the store → recorded as `foreign_links`, left alone.
/// - Existing managed symlink not in `desired` → link removed, recorded as `unlinked`.
/// - Existing real subdirectory → recorded as `untracked`, left untouched (I3).
/// - Desired skill missing from store → recorded as `ghosts`.
/// - Desired skill already linked → idempotent no-op.
/// - Desired skill blocked by a same-named real directory → recorded as `shadowed`.
/// - Otherwise the store skill is linked and recorded as `linked`.
pub fn reconcile_skills(paths: &Paths, persona: &Persona) -> Result<ReconcileReport> {
    let mut report = ReconcileReport::default();

    // Compute the desired skill set: active ∪ {cc-persona}.
    let mut desired: Vec<String> = persona
        .skills
        .as_ref()
        .map(|s| s.active.clone())
        .unwrap_or_default();
    if !desired.iter().any(|n| n == CC_PERSONA_SKILL) {
        desired.push(CC_PERSONA_SKILL.to_string());
    }

    // ~/.claude/skills must be a real directory (I1).
    symlink::ensure_real_dir(&paths.claude_skills)?;

    // Pass 1: classify everything currently in ~/.claude/skills.
    for entry in std::fs::read_dir(&paths.claude_skills)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if is_symlink(&path) {
            let target = std::fs::read_link(&path)?;
            if !target_in_store(&target, &paths.skill_store) {
                report.foreign_links.push(name);
                continue;
            }
            // Managed link: drop it if the skill is no longer desired.
            if !desired.iter().any(|d| d == &name) {
                std::fs::remove_file(&path)
                    .with_context(|| format!("Failed to unlink {}", path.display()))?;
                report.unlinked.push(name);
            }
        } else if path.is_dir() {
            // Real subdirectory = untracked (wild). Never touched.
            report.untracked.push(name);
        }
    }

    // Pass 2: ensure each desired skill is linked from the store.
    for name in &desired {
        let store_skill = paths.skill_store.join(name);
        if !store_skill.exists() {
            report.ghosts.push(name.clone());
            continue;
        }
        let link = paths.claude_skills.join(name);
        if is_symlink(&link) {
            // Already linked (and target validated as in-store in pass 1).
            continue;
        }
        if link.exists() {
            // Same-named wild directory blocks the link.
            report.shadowed.push(name.clone());
            continue;
        }
        symlink::link_skill(&store_skill, &link)?;
        report.linked.push(name.clone());
    }

    report.linked.sort();
    report.unlinked.sort();
    report.untracked.sort();
    report.ghosts.sort();
    report.shadowed.sort();
    report.foreign_links.sort();
    Ok(report)
}

/// Whether `target` (a symlink target) resolves to a path inside the skill-store.
fn target_in_store(target: &Path, skill_store: &Path) -> bool {
    let resolved = if target.is_absolute() {
        target.to_path_buf()
    } else {
        // Relative symlink targets are anchored at ~/.claude/skills.
        // Compare against the store path as-is; canonicalize when possible.
        return std::fs::canonicalize(target)
            .ok()
            .zip(std::fs::canonicalize(skill_store).ok())
            .map(|(t, store)| t.starts_with(&store))
            .unwrap_or(false);
    };
    match (
        std::fs::canonicalize(&resolved).ok(),
        std::fs::canonicalize(skill_store).ok(),
    ) {
        (Some(t), Some(store)) => t.starts_with(&store),
        _ => resolved.starts_with(skill_store),
    }
}

/// List skills in a directory, annotated with `managed` (symlink) and `disabled`.
///
/// `managed` is true when the entry is a symlink (a managed link into the store);
/// a real subdirectory is untracked. `disabled` reads the SKILL.md frontmatter.
pub fn list_skills_ext(skills_dir: &Path) -> Result<Vec<SkillEntry>> {
    let mut result = Vec::new();
    if !skills_dir.exists() {
        return Ok(result);
    }

    for entry in std::fs::read_dir(skills_dir)? {
        let entry = entry?;
        let path = entry.path();
        let managed = is_symlink(&path);
        // Follow symlinks for the dir check so managed links still count.
        if !path.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }

        let content = std::fs::read_to_string(&skill_md)?;
        let disabled = is_disabled(&content);
        result.push(SkillEntry {
            name,
            disabled,
            managed,
        });
    }

    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

/// Switch the skills symlink to point to the given persona's skill-set.
/// If ~/.claude/skills is a real directory (first-time use), auto-migrate it
/// into the target persona's skill-set.
///
/// Retired from the main path in favour of [`reconcile_skills`]; retained for
/// backward-compatible tests and the legacy whole-directory-symlink model.
#[allow(dead_code)]
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

/// Resolve the effective skills directory.
///
/// Legacy helper: under the new model `~/.claude/skills` is always a real directory
/// (I1), so callers read it directly. Retained for the backward-compatible
/// whole-directory-symlink path and tests.
#[allow(dead_code)]
pub fn resolve_skills_dir(paths: &Paths) -> Result<PathBuf> {
    if paths.claude_skills.is_symlink() {
        Ok(std::fs::read_link(&paths.claude_skills)?)
    } else {
        Ok(paths.claude_skills.clone())
    }
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
///
/// Retired from the main path in favour of [`reconcile_skills`]; toggling is now
/// an explicit, optional "temporary mute" via `toggle_skill`.
#[allow(dead_code)]
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

/// Remove a stale `disable-model-invocation` flag from a SKILL.md, if present.
///
/// Used when ingesting skills into the store: the new model decides activation by
/// link presence, not by per-file mute flags, so legacy disables are cleared.
pub fn clear_disable_flag(skill_md: &Path) -> Result<()> {
    if !skill_md.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(skill_md)?;
    if is_disabled(&content) {
        set_skill_disabled(skill_md, false)?;
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
///
/// Legacy helper superseded by [`list_skills_ext`] (which also reports whether each
/// entry is managed vs untracked). Retained for compatibility.
#[allow(dead_code)]
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
///
/// Legacy whole-directory removal. New code should compose [`unlink_skill`] (to
/// detach the managed link under `~/.claude/skills`) with [`remove_from_store`]
/// (to delete the single shared copy) instead.
#[allow(dead_code)]
pub fn remove_skill(skills_dir: &Path, name: &str) -> Result<()> {
    let skill_dir = skills_dir.join(name);
    if skill_dir.exists() {
        std::fs::remove_dir_all(&skill_dir)
            .with_context(|| format!("Failed to remove skill: {}", name))?;
    }
    Ok(())
}

/// Detach a managed link under `~/.claude/skills`.
///
/// Only removes the entry when it is a symlink (a managed link); a real directory
/// is untracked (I3) and is left untouched. Missing paths are a no-op.
pub fn unlink_skill(link: &Path) -> Result<()> {
    if is_symlink(link) {
        std::fs::remove_file(link)
            .with_context(|| format!("Failed to unlink {}", link.display()))?;
    }
    Ok(())
}

/// Delete a skill's single shared copy from the store.
///
/// This affects every persona that references the skill (I5: one copy, shared).
pub fn remove_from_store(store_skill: &Path) -> Result<()> {
    if store_skill.exists() {
        std::fs::remove_dir_all(store_skill)
            .with_context(|| format!("Failed to remove store skill: {}", store_skill.display()))?;
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

    #[cfg(unix)]
    fn persona_with_active(active: &[&str]) -> Persona {
        Persona {
            skills: Some(SkillsConfig {
                active: active.iter().map(|s| s.to_string()).collect(),
            }),
            ..Default::default()
        }
    }

    #[cfg(unix)]
    fn read_link_name(path: &Path) -> String {
        std::fs::read_link(path)
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string()
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_links_only_active_skills_plus_cc_persona() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.create_store_skill("beta", "---\nname: beta\n---\n");
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");

        let report = reconcile_skills(&env.paths, &persona_with_active(&["alpha"])).unwrap();

        assert_eq!(report.linked, vec!["alpha", "cc-persona"]);
        assert!(env.paths.claude_skills.join("alpha").is_symlink());
        assert!(env.paths.claude_skills.join("cc-persona").is_symlink());
        assert!(!env.paths.claude_skills.join("beta").exists());
        assert_eq!(
            read_link_name(&env.paths.claude_skills.join("alpha")),
            "alpha"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_unlinks_skill_removed_from_active() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");
        reconcile_skills(&env.paths, &persona_with_active(&["alpha"])).unwrap();
        assert!(env.paths.claude_skills.join("alpha").is_symlink());

        let report = reconcile_skills(&env.paths, &persona_with_active(&[])).unwrap();

        assert_eq!(report.unlinked, vec!["alpha"]);
        assert!(!env.paths.claude_skills.join("alpha").exists());
        // cc-persona is always desired, so it remains linked.
        assert!(env.paths.claude_skills.join("cc-persona").is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_preserves_wild_real_directory_as_untracked() {
        // Core bypass scenario: an external installer dropped a real skill
        // directory into ~/.claude/skills. Reconcile must classify it as
        // untracked and leave it physically untouched (invariant I3).
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");
        let wild = env.paths.claude_skills.join("wild");
        std::fs::create_dir_all(&wild).unwrap();
        env.write_file(&wild.join("SKILL.md"), "---\nname: wild\n---\nBody\n");

        let report = reconcile_skills(&env.paths, &persona_with_active(&[])).unwrap();

        assert_eq!(report.untracked, vec!["wild"]);
        // Untouched: still a real directory with its original contents.
        assert!(!wild.is_symlink());
        assert!(wild.is_dir());
        assert_eq!(
            env.read_file(&wild.join("SKILL.md")),
            "---\nname: wild\n---\nBody\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_records_ghost_when_store_dir_missing() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");

        // "missing" is desired but has no store directory.
        let report = reconcile_skills(&env.paths, &persona_with_active(&["missing"])).unwrap();

        assert_eq!(report.ghosts, vec!["missing"]);
        assert!(!env.paths.claude_skills.join("missing").exists());
        // Never bails on a ghost.
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_records_shadowed_when_wild_dir_blocks_desired_link() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");
        // A wild real directory occupies the name we want to link.
        let blocker = env.paths.claude_skills.join("alpha");
        std::fs::create_dir_all(&blocker).unwrap();
        env.write_file(&blocker.join("SKILL.md"), "wild alpha");

        let report = reconcile_skills(&env.paths, &persona_with_active(&["alpha"])).unwrap();

        assert_eq!(report.shadowed, vec!["alpha"]);
        // The wild directory is not overwritten.
        assert!(!blocker.is_symlink());
        assert_eq!(env.read_file(&blocker.join("SKILL.md")), "wild alpha");
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_records_foreign_link_pointing_outside_store() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");
        // A symlink whose target escapes the store. ~/.claude/skills is created
        // lazily, so materialize it before planting the foreign link.
        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();
        let outside = env.paths.root.join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        let foreign = env.paths.claude_skills.join("foreign");
        std::os::unix::fs::symlink(&outside, &foreign).unwrap();

        let report = reconcile_skills(&env.paths, &persona_with_active(&[])).unwrap();

        assert_eq!(report.foreign_links, vec!["foreign"]);
        // Foreign links are left alone, never removed.
        assert!(foreign.is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_is_idempotent() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");

        let first = reconcile_skills(&env.paths, &persona_with_active(&["alpha"])).unwrap();
        assert_eq!(first.linked, vec!["alpha", "cc-persona"]);

        let second = reconcile_skills(&env.paths, &persona_with_active(&["alpha"])).unwrap();
        // Second run links nothing new and unlinks nothing.
        assert!(second.linked.is_empty());
        assert!(second.unlinked.is_empty());
        assert!(env.paths.claude_skills.join("alpha").is_symlink());
        assert!(env.paths.claude_skills.join("cc-persona").is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_always_links_cc_persona_even_when_not_in_active() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");

        let report = reconcile_skills(&env.paths, &persona_with_active(&[])).unwrap();

        assert_eq!(report.linked, vec!["cc-persona"]);
        assert!(env.paths.claude_skills.join("cc-persona").is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_converts_legacy_symlink_dir_into_real_dir() {
        // I1: if ~/.claude/skills is a legacy whole-directory symlink, reconcile
        // (via ensure_real_dir) replaces it with a real directory before linking.
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");
        // Make ~/.claude/skills a symlink to some other directory.
        std::fs::remove_dir_all(&env.paths.claude_skills).ok();
        let legacy_target = env.paths.root.join("legacy-skillset");
        std::fs::create_dir_all(&legacy_target).unwrap();
        std::os::unix::fs::symlink(&legacy_target, &env.paths.claude_skills).unwrap();
        assert!(env.paths.claude_skills.is_symlink());

        reconcile_skills(&env.paths, &persona_with_active(&[])).unwrap();

        assert!(!env.paths.claude_skills.is_symlink());
        assert!(env.paths.claude_skills.is_dir());
        assert!(env.paths.claude_skills.join("cc-persona").is_symlink());
    }
}
