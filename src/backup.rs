use anyhow::{Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::Paths;
use crate::symlink::{self, is_symlink};

/// On-disk manifest of the managed per-skill links under `~/.claude/skills`.
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

/// Create a backup of current Claude Code config before switching.
/// Returns the backup directory path.
pub fn create_backup(paths: &Paths) -> Result<PathBuf> {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S");
    let backup_dir = paths.backups.join(format!("pre-switch-{}", timestamp));
    std::fs::create_dir_all(&backup_dir)?;

    // Backup settings.json
    if paths.claude_settings.exists() {
        std::fs::copy(&paths.claude_settings, backup_dir.join("settings.json"))?;
    }

    // Backup ~/.claude.json (MCP config)
    if paths.claude_json.exists() {
        std::fs::copy(&paths.claude_json, backup_dir.join("claude.json"))?;
    }

    // Backup skills state.
    if paths.claude_skills.exists() || is_symlink(&paths.claude_skills) {
        let meta = std::fs::symlink_metadata(&paths.claude_skills)?;
        if meta.file_type().is_symlink() {
            // Legacy whole-directory symlink model: record the target so a
            // restore can recreate it (kept for backward compatibility).
            let target = std::fs::read_link(&paths.claude_skills)?;
            std::fs::write(
                backup_dir.join("skills-symlink"),
                target.to_string_lossy().as_bytes(),
            )?;
        } else {
            // New model: `~/.claude/skills` is a real directory holding per-skill
            // symlinks into the store + wild real subdirectories. Record the
            // managed links (by store-relative target) and the wild names.
            let manifest = snapshot_skill_links(paths)?;
            let content = serde_json::to_string_pretty(&manifest)?;
            std::fs::write(backup_dir.join("skills-links.json"), content)?;
        }
    }

    // Backup CLAUDE.md symlink or file
    if paths.claude_md_file.exists() || is_symlink(&paths.claude_md_file) {
        let meta = std::fs::symlink_metadata(&paths.claude_md_file)?;
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&paths.claude_md_file)?;
            std::fs::write(
                backup_dir.join("claude-md-symlink"),
                target.to_string_lossy().as_bytes(),
            )?;
        } else {
            std::fs::copy(&paths.claude_md_file, backup_dir.join("CLAUDE.md"))?;
        }
    }

    Ok(backup_dir)
}

/// Capture the current managed per-skill links + untracked wild directories under
/// `~/.claude/skills` into a [`SkillLinksManifest`].
///
/// Managed links (symlinks whose target resolves inside the store) record their
/// store-relative target. Wild real subdirectories are recorded by name only.
/// When `~/.claude/skills` is itself a symlink (legacy model) or missing, an empty
/// manifest is returned — the legacy backup path handles that case separately.
pub fn snapshot_skill_links(paths: &Paths) -> Result<SkillLinksManifest> {
    let mut manifest = SkillLinksManifest::default();
    if !paths.claude_skills.exists() || is_symlink(&paths.claude_skills) {
        return Ok(manifest);
    }

    for entry in std::fs::read_dir(&paths.claude_skills)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if is_symlink(&path) {
            let target = std::fs::read_link(&path)?;
            match store_relative_target(&target, &paths.skill_store) {
                Some(rel) => {
                    manifest.links.insert(name, rel);
                }
                None => {
                    // Foreign symlink (points outside the store) — not managed by
                    // cc-persona; do not record it as a restorable link.
                }
            }
        } else if path.is_dir() {
            manifest.untracked.push(name);
        }
    }

    manifest.untracked.sort();
    Ok(manifest)
}

/// Rebuild the managed per-skill links described by `manifest` under
/// `~/.claude/skills`.
///
/// Ensures `~/.claude/skills` is a real directory first, removes every currently
/// managed symlink (so the restore is exact), then recreates each link from the
/// store. A manifest entry whose store target no longer exists is warned about and
/// skipped. Wild/untracked real subdirectories are never touched.
pub fn restore_skill_links(paths: &Paths, manifest: &SkillLinksManifest) -> Result<()> {
    symlink::ensure_real_dir(&paths.claude_skills)?;

    // Drop every currently-present managed symlink so the restore is exact.
    for entry in std::fs::read_dir(&paths.claude_skills)? {
        let entry = entry?;
        let path = entry.path();
        if is_symlink(&path) {
            std::fs::remove_file(&path)?;
        }
    }

    // Recreate links from the manifest.
    for (name, rel_target) in &manifest.links {
        let store_skill = paths.skill_store.join(rel_target);
        if !store_skill.exists() {
            eprintln!(
                "  ⚠ Skipping link '{}': store skill '{}' no longer exists.",
                name,
                store_skill.display()
            );
            continue;
        }
        let link = paths.claude_skills.join(name);
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

/// Restore from the most recent backup.
pub fn restore_latest(paths: &Paths) -> Result<()> {
    let latest = find_latest_backup(&paths.backups)?;
    let backup_dir = match latest {
        Some(d) => d,
        None => bail!("No backup found. Nothing to restore."),
    };

    eprintln!("Restoring from: {}", backup_dir.display());

    // Restore settings.json
    let backup_settings = backup_dir.join("settings.json");
    if backup_settings.exists() {
        std::fs::copy(&backup_settings, &paths.claude_settings)?;
    }

    // Restore ~/.claude.json
    let backup_claude_json = backup_dir.join("claude.json");
    if backup_claude_json.exists() {
        std::fs::copy(&backup_claude_json, &paths.claude_json)?;
    }

    // Restore skills. Three on-disk formats, newest first:
    //   skills-links.json — new model: managed per-skill links + untracked names.
    //   skills-symlink    — legacy whole-directory symlink target.
    //   skills-is-dir     — legacy marker that skills was a plain real directory.
    let skills_links_file = backup_dir.join("skills-links.json");
    let skills_symlink_file = backup_dir.join("skills-symlink");
    let skills_is_dir = backup_dir.join("skills-is-dir");
    if skills_links_file.exists() {
        let content = std::fs::read_to_string(&skills_links_file)?;
        let manifest: SkillLinksManifest = serde_json::from_str(&content)?;
        restore_skill_links(paths, &manifest)?;
    } else if skills_symlink_file.exists() {
        let target = std::fs::read_to_string(&skills_symlink_file)?;
        remove_symlink_or_dir(&paths.claude_skills)?;
        std::os::unix::fs::symlink(target.trim(), &paths.claude_skills)?;
    } else if skills_is_dir.exists() {
        // Was a real directory before migration. The contents have been migrated
        // into the persona's skill-set. Restore by keeping the symlink pointing
        // to the same skill-set (content is there), or if no symlink exists,
        // recreate the directory from the skill-set that received the migration.
        // The simplest safe approach: leave the current symlink as-is since the
        // skills content is managed by cc-persona now.
        eprintln!("  Note: skills directory was migrated into cc-persona management.");
        eprintln!("  Skills remain accessible via the current skill-set symlink.");
    }

    // Restore CLAUDE.md
    let claude_md_symlink_file = backup_dir.join("claude-md-symlink");
    let claude_md_backup = backup_dir.join("CLAUDE.md");
    if claude_md_symlink_file.exists() {
        let target = std::fs::read_to_string(&claude_md_symlink_file)?;
        remove_symlink_or_file(&paths.claude_md_file)?;
        std::os::unix::fs::symlink(target.trim(), &paths.claude_md_file)?;
    } else if claude_md_backup.exists() {
        remove_symlink_or_file(&paths.claude_md_file)?;
        std::fs::copy(&claude_md_backup, &paths.claude_md_file)?;
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

/// Express a symlink target as a path relative to the skill-store, if it resolves
/// inside the store. Returns `None` for foreign targets.
fn store_relative_target(target: &Path, skill_store: &Path) -> Option<String> {
    let (resolved_target, resolved_store) = match (
        std::fs::canonicalize(target).ok(),
        std::fs::canonicalize(skill_store).ok(),
    ) {
        (Some(t), Some(s)) => (t, s),
        _ => {
            // Fall back to lexical comparison when canonicalization fails.
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

        let manifest = snapshot_skill_links(&env.paths).unwrap();

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

        // Snapshot the current state into a backup (writes skills-links.json).
        let backup_dir = create_backup(&env.paths).unwrap();
        assert!(backup_dir.join("skills-links.json").exists());

        // Drift away from the snapshot: drop beta, add a stray link to gamma.
        std::fs::remove_file(env.paths.claude_skills.join("beta")).unwrap();
        env.create_store_skill("gamma", "---\nname: gamma\n---\n");
        env.link_into_claude_skills("gamma");

        restore_latest(&env.paths).unwrap();

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

        restore_skill_links(&env.paths, &manifest).unwrap();

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
        // ensure_dirs does not create ~/.claude/skills (it is created lazily), so
        // the legacy restore is free to recreate it as a whole-directory symlink.
        assert!(!env.paths.claude_skills.exists());

        restore_latest(&env.paths).unwrap();

        assert!(env.paths.claude_skills.is_symlink());
        assert_eq!(
            std::fs::read_link(&env.paths.claude_skills).unwrap(),
            legacy_target
        );
    }
}
