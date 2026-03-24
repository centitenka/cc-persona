use anyhow::{Result, bail};
use chrono::Local;
use std::path::{Path, PathBuf};

use crate::config::Paths;

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

    // Backup skills symlink target or directory info
    if paths.claude_skills.exists() {
        let meta = std::fs::symlink_metadata(&paths.claude_skills)?;
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&paths.claude_skills)?;
            std::fs::write(
                backup_dir.join("skills-symlink"),
                target.to_string_lossy().as_bytes(),
            )?;
        } else {
            // It's a real directory — record this fact
            std::fs::write(backup_dir.join("skills-is-dir"), "true")?;
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

    // Restore skills symlink
    let skills_symlink_file = backup_dir.join("skills-symlink");
    let skills_is_dir = backup_dir.join("skills-is-dir");
    if skills_symlink_file.exists() {
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

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
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
}
