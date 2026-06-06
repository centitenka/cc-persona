use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::claude::{mcp, settings};
use crate::config::Paths;
use crate::persona::{self, Persona};
use crate::symlink::is_symlink;

/// The cc-persona skill is always managed and is excluded from untracked listings.
const CC_PERSONA_SKILL: &str = "cc-persona";

/// Read-only drift report comparing `~/.claude/skills` against the active persona.
///
/// Every field is purely diagnostic; computing this never mutates the filesystem.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DriftReport {
    /// Real subdirectories under `~/.claude/skills` not managed by cc-persona.
    pub untracked: Vec<String>,
    /// Desired skills (active ∪ {cc-persona}) with no directory in the store.
    pub ghosts: Vec<String>,
    /// Desired skills missing their managed link under `~/.claude/skills`.
    pub drifted_missing_link: Vec<String>,
    /// Managed links present that the active persona does not desire.
    pub drifted_extra_link: Vec<String>,
    /// Symlinks under `~/.claude/skills` pointing outside the store.
    pub foreign_links: Vec<String>,
    /// Whether `~/.claude/skills` itself is a symlink (legacy whole-dir model, I1 violated).
    pub skills_dir_is_symlink: bool,
}

/// Inspect `~/.claude/skills` against the (optional) active persona.
///
/// Read-only: classifies every entry and computes link drift relative to the
/// desired set (`active ∪ {cc-persona}`). When `active` is `None`, only the
/// untracked / foreign / symlink-dir facts are meaningful; desired-set drift is
/// computed against just `{cc-persona}`.
pub fn inspect_skills(paths: &Paths, active: Option<&Persona>) -> Result<DriftReport> {
    let mut report = DriftReport {
        skills_dir_is_symlink: is_symlink(&paths.claude_skills),
        ..Default::default()
    };

    // Desired = active ∪ {cc-persona}.
    let mut desired: Vec<String> = active
        .and_then(|p| p.skills.as_ref())
        .map(|s| s.active.clone())
        .unwrap_or_default();
    if !desired.iter().any(|n| n == CC_PERSONA_SKILL) {
        desired.push(CC_PERSONA_SKILL.to_string());
    }

    // If the skills dir is a symlink (legacy model), we cannot meaningfully read
    // per-skill links; record the fact and return early.
    if report.skills_dir_is_symlink || !paths.claude_skills.exists() {
        // Ghosts can still be computed from the store regardless of the links dir.
        for name in &desired {
            if !paths.skill_store.join(name).exists() {
                report.ghosts.push(name.clone());
            }
        }
        report.ghosts.sort();
        return Ok(report);
    }

    // Managed links currently present (symlink → store).
    let managed = list_managed_links(paths)?;
    let managed_names: Vec<String> = managed.iter().map(|(n, _)| n.clone()).collect();

    // Classify entries under ~/.claude/skills.
    for entry in std::fs::read_dir(&paths.claude_skills)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if is_symlink(&path) {
            let target = std::fs::read_link(&path)?;
            if !target_in_store(&target, paths) {
                report.foreign_links.push(name);
                continue;
            }
            // Managed link not desired → extra.
            if !desired.iter().any(|d| d == &name) {
                report.drifted_extra_link.push(name);
            }
        } else if path.is_dir() && name != CC_PERSONA_SKILL {
            report.untracked.push(name);
        }
    }

    // Desired skills: missing-link or ghost.
    for name in &desired {
        if !paths.skill_store.join(name).exists() {
            report.ghosts.push(name.clone());
            continue;
        }
        if !managed_names.iter().any(|m| m == name) {
            report.drifted_missing_link.push(name.clone());
        }
    }

    report.untracked.sort();
    report.ghosts.sort();
    report.drifted_missing_link.sort();
    report.drifted_extra_link.sort();
    report.foreign_links.sort();
    Ok(report)
}

/// List names of untracked skills: real subdirectories under `~/.claude/skills`,
/// excluding symlinks and `cc-persona`.
pub fn list_untracked_skills(paths: &Paths) -> Result<Vec<String>> {
    let mut names = Vec::new();
    if !paths.claude_skills.exists() {
        return Ok(names);
    }
    for entry in std::fs::read_dir(&paths.claude_skills)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if is_symlink(&path) {
            continue;
        }
        if name == CC_PERSONA_SKILL {
            continue;
        }
        if path.is_dir() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// List managed links under `~/.claude/skills`: `(name, symlink_target)` pairs
/// for each symlink whose target resolves inside the skill-store.
pub fn list_managed_links(paths: &Paths) -> Result<Vec<(String, PathBuf)>> {
    let mut links = Vec::new();
    if !paths.claude_skills.exists() || is_symlink(&paths.claude_skills) {
        return Ok(links);
    }
    for entry in std::fs::read_dir(&paths.claude_skills)? {
        let entry = entry?;
        let path = entry.path();
        if !is_symlink(&path) {
            continue;
        }
        let target = std::fs::read_link(&path)?;
        if !target_in_store(&target, paths) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        links.push((name, target));
    }
    links.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(links)
}

/// Plugins enabled in `settings.json` (`enabledPlugins[name] == true`) that no
/// persona declares in `[settings].enabledPlugins`.
pub fn untracked_plugins(paths: &Paths) -> Result<Vec<String>> {
    let live = settings::read_settings(&paths.claude_settings)?;
    let mut enabled = Vec::new();
    if let Some(map) = live.get("enabledPlugins").and_then(|v| v.as_object()) {
        for (name, value) in map {
            if value.as_bool().unwrap_or(false) {
                enabled.push(name.clone());
            }
        }
    }

    let declared = declared_plugins(paths)?;
    let mut untracked: Vec<String> = enabled
        .into_iter()
        .filter(|name| !declared.iter().any(|d| d == name))
        .collect();
    untracked.sort();
    untracked.dedup();
    Ok(untracked)
}

/// MCP server names present in `~/.claude.json` that no persona references via
/// `[mcp].enable`/`[mcp].disable` (substring match, mirroring apply semantics).
pub fn untracked_mcp(paths: &Paths) -> Result<Vec<String>> {
    let servers = mcp::list_mcp_servers(&paths.claude_json)?;
    let patterns = declared_mcp_patterns(paths)?;
    let mut untracked: Vec<String> = servers
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| !patterns.iter().any(|p| name.contains(p.as_str())))
        .collect();
    untracked.sort();
    untracked.dedup();
    Ok(untracked)
}

/// Total `(count, bytes)` of all backup directories under `~/.cc-persona/backups`.
pub fn backups_usage(paths: &Paths) -> Result<(usize, u64)> {
    if !paths.backups.exists() {
        return Ok((0, 0));
    }
    let mut count = 0usize;
    let mut bytes = 0u64;
    for entry in std::fs::read_dir(&paths.backups)? {
        let entry = entry?;
        if entry.path().is_dir() {
            count += 1;
            bytes += dir_size(&entry.path())?;
        }
    }
    Ok((count, bytes))
}

/// Whether the active-persona snapshot is missing (dirty-guard would be dumb).
pub fn snapshot_missing(paths: &Paths) -> bool {
    !paths.active_persona_state.exists()
}

/// Collect every plugin name declared `true` across all persona `[settings].enabledPlugins`.
fn declared_plugins(paths: &Paths) -> Result<Vec<String>> {
    let mut declared = Vec::new();
    for name in persona::list_personas(&paths.personas)? {
        let path = persona::persona_path(&paths.personas, &name);
        let Ok(p) = Persona::load(&path) else {
            continue;
        };
        if let Some(map) = p
            .settings
            .as_ref()
            .and_then(|s| s.get("enabledPlugins"))
            .and_then(|v| v.as_object())
        {
            for (plugin, value) in map {
                if value.as_bool().unwrap_or(false) {
                    declared.push(plugin.clone());
                }
            }
        }
    }
    declared.sort();
    declared.dedup();
    Ok(declared)
}

/// Collect every MCP enable/disable pattern across all persona `[mcp]` sections.
fn declared_mcp_patterns(paths: &Paths) -> Result<Vec<String>> {
    let mut patterns = Vec::new();
    for name in persona::list_personas(&paths.personas)? {
        let path = persona::persona_path(&paths.personas, &name);
        let Ok(p) = Persona::load(&path) else {
            continue;
        };
        if let Some(mcp) = p.mcp.as_ref() {
            patterns.extend(mcp.enable.iter().cloned());
            patterns.extend(mcp.disable.iter().cloned());
        }
    }
    patterns.sort();
    patterns.dedup();
    Ok(patterns)
}

/// Whether a symlink `target` resolves inside the skill-store.
fn target_in_store(target: &std::path::Path, paths: &Paths) -> bool {
    match (
        std::fs::canonicalize(target).ok(),
        std::fs::canonicalize(&paths.skill_store).ok(),
    ) {
        (Some(t), Some(store)) => t.starts_with(&store),
        _ => target.starts_with(&paths.skill_store),
    }
}

fn dir_size(dir: &std::path::Path) -> Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            total += dir_size(&entry.path())?;
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;

    #[cfg(unix)]
    #[test]
    fn list_untracked_skills_excludes_symlinks_and_cc_persona() {
        let env = TestEnv::new();
        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();
        env.create_skill(&env.paths.claude_skills, "wild", "---\nname: wild\n---\n");
        env.create_skill(
            &env.paths.claude_skills,
            "cc-persona",
            "---\nname: cc-persona\n---\n",
        );
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.link_into_claude_skills("alpha");

        let untracked = list_untracked_skills(&env.paths).unwrap();
        assert_eq!(untracked, vec!["wild".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn inspect_skills_reports_legacy_symlink_dir() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        let target = env.paths.skill_sets.join("engineer");
        std::fs::create_dir_all(&target).unwrap();
        env.symlink(&target, &env.paths.claude_skills);

        let report = inspect_skills(&env.paths, None).unwrap();
        assert!(report.skills_dir_is_symlink);
    }
}
