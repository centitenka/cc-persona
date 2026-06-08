use anyhow::{Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::claude::mcp;
use crate::config::Target;
use crate::symlink::{self, is_symlink};

/// On-disk manifest of the managed per-skill links under a `skills/` directory.
///
/// Written as `skills-links.json` inside a backup directory. Records each managed
/// link's store-relative target (so restore can rebuild it from the shared store)
/// and the names of wild/untracked real subdirectories (recorded by name only —
/// their contents are never copied into the backup, they stay in place on restore).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLinksManifest {
    /// Managed link name → store-relative target (e.g. `"alpha" -> "alpha"`).
    #[serde(default)]
    pub links: BTreeMap<String, String>,
    /// Untracked (wild) real subdirectory names, recorded for diagnostics only.
    #[serde(default)]
    pub untracked: Vec<String>,
}

/// Which targets existed before the switch, so restore can DELETE the ones
/// cc-persona created (project `settings.local.json` / `skills/` usually did not
/// exist before the first `use --project`) rather than leaving empty husks.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct TargetPresence {
    #[serde(default)]
    settings: bool,
    #[serde(default)]
    skills_dir: bool,
    #[serde(default)]
    claude_md: bool,
}

/// Backup of the project's connector subtree (`projects.<key>.disabledMcpServers`).
/// `field_present` distinguishes "the field was absent" from "present but empty",
/// so restore can remove what cc-persona added.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct ConnectorBackup {
    #[serde(default)]
    field_present: bool,
    #[serde(default)]
    disabled: Vec<String>,
}

/// Create a backup of the current Claude Code config for `target` before switching.
/// Returns the backup directory path.
pub fn create_backup(target: &Target, skill_store: &Path) -> Result<PathBuf> {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S");
    let backup_dir = target.backups_dir.join(format!("pre-switch-{}", timestamp));
    std::fs::create_dir_all(&backup_dir)?;

    // settings (settings.json at global, settings.local.json at project).
    let mut presence = TargetPresence {
        settings: target.settings_file.exists(),
        ..Default::default()
    };
    if presence.settings {
        std::fs::copy(&target.settings_file, backup_dir.join("settings.json"))?;
    }

    // Skills directory state.
    presence.skills_dir = target.skills_dir.exists() || is_symlink(&target.skills_dir);
    if presence.skills_dir {
        let meta = std::fs::symlink_metadata(&target.skills_dir)?;
        if meta.file_type().is_symlink() {
            // Legacy whole-directory symlink model: record the target.
            let link_target = std::fs::read_link(&target.skills_dir)?;
            std::fs::write(
                backup_dir.join("skills-symlink"),
                link_target.to_string_lossy().as_bytes(),
            )?;
        } else {
            let manifest = snapshot_skill_links(&target.skills_dir, skill_store)?;
            let content = serde_json::to_string_pretty(&manifest)?;
            std::fs::write(backup_dir.join("skills-links.json"), content)?;
        }
    }

    // MCP: global backs up the whole ~/.claude.json; project backs up only its
    // connector subtree (so restoring one project never clobbers another).
    match &target.claude_json_project_key {
        None => {
            if target.claude_json.exists() {
                std::fs::copy(&target.claude_json, backup_dir.join("claude.json"))?;
            }
        }
        Some(key) => {
            let json = mcp::read_claude_json(&target.claude_json)?;
            let disabled = mcp::read_project_disabled(&json, key);
            let backup = ConnectorBackup {
                field_present: disabled.is_some(),
                disabled: disabled.unwrap_or_default(),
            };
            std::fs::write(
                backup_dir.join("connectors.json"),
                serde_json::to_string_pretty(&backup)?,
            )?;
        }
    }

    // CLAUDE.md — only when this scope manages it.
    if let Some(md_file) = &target.claude_md_file {
        presence.claude_md = md_file.exists() || is_symlink(md_file);
        if presence.claude_md {
            let meta = std::fs::symlink_metadata(md_file)?;
            if meta.file_type().is_symlink() {
                let link_target = std::fs::read_link(md_file)?;
                std::fs::write(
                    backup_dir.join("claude-md-symlink"),
                    link_target.to_string_lossy().as_bytes(),
                )?;
            } else {
                std::fs::copy(md_file, backup_dir.join("CLAUDE.md"))?;
            }
        }
    }

    std::fs::write(
        backup_dir.join("target-presence.json"),
        serde_json::to_string_pretty(&presence)?,
    )?;

    Ok(backup_dir)
}

/// Capture the managed per-skill links + untracked wild directories under
/// `skills_dir` into a [`SkillLinksManifest`].
///
/// Managed links (symlinks whose target resolves inside the store) record their
/// store-relative target. Wild real subdirectories are recorded by name only.
/// When `skills_dir` is itself a symlink (legacy model) or missing, an empty
/// manifest is returned.
pub fn snapshot_skill_links(skills_dir: &Path, skill_store: &Path) -> Result<SkillLinksManifest> {
    let mut manifest = SkillLinksManifest::default();
    if !skills_dir.exists() || is_symlink(skills_dir) {
        return Ok(manifest);
    }

    for entry in std::fs::read_dir(skills_dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if is_symlink(&path) {
            let target = std::fs::read_link(&path)?;
            match store_relative_target(&target, skill_store) {
                Some(rel) => {
                    manifest.links.insert(name, rel);
                }
                None => {
                    // Foreign symlink (points outside the store) — not managed.
                }
            }
        } else if path.is_dir() {
            manifest.untracked.push(name);
        }
    }

    manifest.untracked.sort();
    Ok(manifest)
}

/// Rebuild the managed per-skill links described by `manifest` under `skills_dir`.
///
/// Ensures `skills_dir` is a real directory first, removes every currently managed
/// symlink (so the restore is exact), then recreates each link from the store. A
/// manifest entry whose store target no longer exists is warned about and skipped.
/// Wild/untracked real subdirectories are never touched.
pub fn restore_skill_links(
    skills_dir: &Path,
    skill_store: &Path,
    manifest: &SkillLinksManifest,
) -> Result<()> {
    symlink::ensure_real_dir(skills_dir)?;

    // Drop every currently-present managed symlink so the restore is exact.
    for entry in std::fs::read_dir(skills_dir)? {
        let entry = entry?;
        let path = entry.path();
        if is_symlink(&path) {
            std::fs::remove_file(&path)?;
        }
    }

    // Recreate links from the manifest.
    for (name, rel_target) in &manifest.links {
        let store_skill = skill_store.join(rel_target);
        if !store_skill.exists() {
            eprintln!(
                "  ⚠ Skipping link '{}': store skill '{}' no longer exists.",
                name,
                store_skill.display()
            );
            continue;
        }
        let link = skills_dir.join(name);
        if link.exists() || is_symlink(&link) {
            // A wild real directory (or stray entry) already occupies this name;
            // do not clobber it (I3).
            if !is_symlink(&link) && link.is_dir() {
                continue;
            }
            std::fs::remove_file(&link).ok();
        }
        symlink::link_skill(&store_skill, &link)?;
    }

    Ok(())
}

/// Restore `target` from its most recent backup. `lock_path` guards the
/// `~/.claude.json` connector write at project scope.
pub fn restore_latest(target: &Target, skill_store: &Path, lock_path: &Path) -> Result<()> {
    let latest = find_latest_backup(&target.backups_dir)?;
    let backup_dir = match latest {
        Some(d) => d,
        None => bail!("No backup found. Nothing to restore."),
    };

    eprintln!("Restoring from: {}", backup_dir.display());

    let presence: Option<TargetPresence> = read_json_opt(&backup_dir.join("target-presence.json"));

    // Restore settings.
    let backup_settings = backup_dir.join("settings.json");
    if backup_settings.exists() {
        std::fs::copy(&backup_settings, &target.settings_file)?;
    } else if presence.as_ref().is_some_and(|p| !p.settings) {
        // cc-persona created this file; remove it so no empty husk is left.
        remove_file_if_exists(&target.settings_file)?;
    }

    // Restore skills. Three on-disk formats, newest first.
    let skills_links_file = backup_dir.join("skills-links.json");
    let skills_symlink_file = backup_dir.join("skills-symlink");
    let skills_is_dir = backup_dir.join("skills-is-dir");
    if skills_links_file.exists() {
        let content = std::fs::read_to_string(&skills_links_file)?;
        let manifest: SkillLinksManifest = serde_json::from_str(&content)?;
        restore_skill_links(&target.skills_dir, skill_store, &manifest)?;
    } else if skills_symlink_file.exists() {
        let link_target = std::fs::read_to_string(&skills_symlink_file)?;
        remove_symlink_or_dir(&target.skills_dir)?;
        std::os::unix::fs::symlink(link_target.trim(), &target.skills_dir)?;
    } else if skills_is_dir.exists() {
        eprintln!("  Note: skills directory was migrated into cc-persona management.");
        eprintln!("  Skills remain accessible via the current skill-set symlink.");
    } else if presence.as_ref().is_some_and(|p| !p.skills_dir) {
        // The skills dir did not exist before this switch (typical at project
        // scope's first `use`). Strip the managed links cc-persona added, then
        // remove the now-empty dir — leaving any untracked real subdir in place.
        remove_managed_links(&target.skills_dir, skill_store);
        remove_dir_if_empty(&target.skills_dir);
    }

    // Restore MCP.
    let backup_claude_json = backup_dir.join("claude.json");
    let backup_connectors = backup_dir.join("connectors.json");
    if backup_claude_json.exists() {
        // Global scope: whole-file restore. Route it through the SAME lock the
        // project-scope writers use, so a concurrent window's read-modify-write of
        // projects.<key> is serialized against this overwrite rather than racing it.
        let value = mcp::read_claude_json(&backup_claude_json)?;
        mcp::update_claude_json(&target.claude_json, lock_path, move |json| {
            *json = value;
            Ok(())
        })?;
    } else if let (Some(key), Some(backup)) = (
        target.claude_json_project_key.as_deref(),
        read_json_opt::<ConnectorBackup>(&backup_connectors),
    ) {
        // Project scope: restore only this project's connector subtree.
        mcp::update_claude_json(&target.claude_json, lock_path, |json| {
            let value = if backup.field_present {
                Some(backup.disabled.as_slice())
            } else {
                None
            };
            mcp::set_project_disabled(json, key, value)
        })?;
    }

    // Restore CLAUDE.md (only when this scope manages it).
    if let Some(md_file) = &target.claude_md_file {
        let claude_md_symlink_file = backup_dir.join("claude-md-symlink");
        let claude_md_backup = backup_dir.join("CLAUDE.md");
        if claude_md_symlink_file.exists() {
            let link_target = std::fs::read_to_string(&claude_md_symlink_file)?;
            remove_symlink_or_file(md_file)?;
            std::os::unix::fs::symlink(link_target.trim(), md_file)?;
        } else if claude_md_backup.exists() {
            remove_symlink_or_file(md_file)?;
            std::fs::copy(&claude_md_backup, md_file)?;
        } else if presence.as_ref().is_some_and(|p| !p.claude_md) {
            remove_symlink_or_file(md_file)?;
        }
    }

    Ok(())
}

fn find_latest_backup(backups_dir: &Path) -> Result<Option<PathBuf>> {
    if !backups_dir.exists() {
        return Ok(None);
    }
    let mut entries: Vec<_> = std::fs::read_dir(backups_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    Ok(entries.last().map(|e| e.path()))
}

/// Read+deserialize a JSON file, returning `None` if missing or malformed.
fn read_json_opt<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    if path.exists() || is_symlink(path) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Remove every managed symlink (target resolves inside the store) directly under
/// `dir`. Best-effort; untracked real subdirectories and foreign symlinks are left
/// untouched. Used by delete-on-restore to undo links cc-persona created.
fn remove_managed_links(dir: &Path, skill_store: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_symlink(&path) {
            continue;
        }
        if let Ok(target) = std::fs::read_link(&path) {
            if store_relative_target(&target, skill_store).is_some() {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Remove a directory if it exists and is empty (best-effort — never errors).
fn remove_dir_if_empty(path: &Path) {
    if path.is_dir()
        && std::fs::read_dir(path)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(path);
    }
}

/// Express a symlink target as a path relative to the skill-store, if it resolves
/// inside the store. Returns `None` for foreign targets.
fn store_relative_target(target: &Path, skill_store: &Path) -> Option<String> {
    let (resolved_target, resolved_store) = match (
        std::fs::canonicalize(target).ok(),
        std::fs::canonicalize(skill_store).ok(),
    ) {
        (Some(t), Some(s)) => (t, s),
        _ => {
            let rel = target.strip_prefix(skill_store).ok()?;
            return Some(rel.to_string_lossy().to_string());
        }
    };
    let rel = resolved_target.strip_prefix(&resolved_store).ok()?;
    Some(rel.to_string_lossy().to_string())
}

fn remove_symlink_or_dir(path: &Path) -> Result<()> {
    if !path.exists() && !is_symlink(path) {
        return Ok(());
    }
    if is_symlink(path) {
        std::fs::remove_file(path)?;
    } else if path.is_dir() {
        // Don't delete real directories — that's dangerous
        bail!(
            "{} is a real directory, not a symlink. Please move it manually.",
            path.display()
        );
    }
    Ok(())
}

fn remove_symlink_or_file(path: &Path) -> Result<()> {
    if !path.exists() && !is_symlink(path) {
        return Ok(());
    }
    std::fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;

    #[test]
    fn find_latest_backup_returns_latest_directory() {
        let env = TestEnv::new();
        std::fs::create_dir_all(&env.paths.backups).unwrap();
        std::fs::create_dir_all(env.paths.backups.join("pre-switch-20260101-010101")).unwrap();
        std::fs::create_dir_all(env.paths.backups.join("pre-switch-20260101-020202")).unwrap();
        env.write_file(&env.paths.backups.join("not-a-dir"), "ignore");

        let latest = find_latest_backup(&env.paths.backups).unwrap().unwrap();

        assert_eq!(
            latest.file_name().and_then(|name| name.to_str()),
            Some("pre-switch-20260101-020202")
        );
    }

    #[cfg(unix)]
    #[test]
    fn remove_symlink_or_dir_rejects_real_dirs_and_removes_symlinks() {
        let env = TestEnv::new();
        let real_dir = env.paths.root.join("real-dir");
        std::fs::create_dir_all(&real_dir).unwrap();

        let err = remove_symlink_or_dir(&real_dir).unwrap_err();
        assert!(format!("{err:#}").contains("real directory"));

        let target = env.paths.root.join("target");
        std::fs::create_dir_all(&target).unwrap();
        let link = env.paths.root.join("skills-link");
        env.symlink(&target, &link);

        remove_symlink_or_dir(&link).unwrap();

        assert!(!link.exists());
        assert!(!is_symlink(&link));
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_skill_links_captures_managed_links_and_wild_dirs_only() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.link_into_claude_skills("alpha");
        // A wild real directory: recorded by name only.
        let wild = env.paths.claude_skills.join("wild");
        std::fs::create_dir_all(&wild).unwrap();
        env.write_file(&wild.join("SKILL.md"), "wild body");
        // A foreign symlink (target outside store): not recorded at all.
        let outside = env.paths.root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, env.paths.claude_skills.join("foreign")).unwrap();

        let manifest =
            snapshot_skill_links(&env.paths.claude_skills, &env.paths.skill_store).unwrap();

        assert_eq!(
            manifest.links.get("alpha").map(String::as_str),
            Some("alpha")
        );
        assert!(!manifest.links.contains_key("foreign"));
        assert_eq!(manifest.untracked, vec!["wild"]);
    }

    #[cfg(unix)]
    #[test]
    fn create_then_restore_round_trips_managed_links_and_preserves_wild() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.create_store_skill("beta", "---\nname: beta\n---\n");
        env.link_into_claude_skills("alpha");
        env.link_into_claude_skills("beta");
        // A wild real directory that must survive the round trip untouched.
        let wild = env.paths.claude_skills.join("wild");
        std::fs::create_dir_all(&wild).unwrap();
        env.write_file(&wild.join("SKILL.md"), "wild body");

        let target = env.global_target();
        // Snapshot the current state into a backup (writes skills-links.json).
        let backup_dir = create_backup(&target, &env.paths.skill_store).unwrap();
        assert!(backup_dir.join("skills-links.json").exists());

        // Drift away from the snapshot: drop beta, add a stray link to gamma.
        std::fs::remove_file(env.paths.claude_skills.join("beta")).unwrap();
        env.create_store_skill("gamma", "---\nname: gamma\n---\n");
        env.link_into_claude_skills("gamma");

        let lock = env.paths.root.join("claude-json.lock");
        restore_latest(&target, &env.paths.skill_store, &lock).unwrap();

        // Managed links match the snapshot exactly: alpha + beta back, gamma gone.
        assert!(env.paths.claude_skills.join("alpha").is_symlink());
        assert!(env.paths.claude_skills.join("beta").is_symlink());
        assert!(!env.paths.claude_skills.join("gamma").exists());
        // The wild real directory is preserved, contents intact.
        assert!(!wild.is_symlink());
        assert!(wild.is_dir());
        assert_eq!(env.read_file(&wild.join("SKILL.md")), "wild body");
    }

    #[cfg(unix)]
    #[test]
    fn restore_skill_links_skips_entry_when_store_target_missing() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        // Manifest references a store skill that does not exist.
        let mut manifest = SkillLinksManifest::default();
        manifest
            .links
            .insert("ghost".to_string(), "ghost".to_string());

        restore_skill_links(&env.paths.claude_skills, &env.paths.skill_store, &manifest).unwrap();

        // The missing target is skipped, not an error, and no link is created.
        assert!(!env.paths.claude_skills.join("ghost").exists());
    }

    #[cfg(unix)]
    #[test]
    fn restore_latest_reads_legacy_skills_symlink_backup() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        // Hand-craft a legacy backup that records a whole-directory symlink target.
        let backup_dir = env.paths.backups.join("pre-switch-20260101-010101");
        std::fs::create_dir_all(&backup_dir).unwrap();
        let legacy_target = env.paths.root.join("legacy-skillset");
        std::fs::create_dir_all(&legacy_target).unwrap();
        env.write_file(
            &backup_dir.join("skills-symlink"),
            &legacy_target.to_string_lossy(),
        );
        // ensure_dirs does not create ~/.claude/skills (it is created lazily).
        assert!(!env.paths.claude_skills.exists());

        let target = env.global_target();
        let lock = env.paths.root.join("claude-json.lock");
        restore_latest(&target, &env.paths.skill_store, &lock).unwrap();

        assert!(env.paths.claude_skills.is_symlink());
        assert_eq!(
            std::fs::read_link(&env.paths.claude_skills).unwrap(),
            legacy_target
        );
    }
}
