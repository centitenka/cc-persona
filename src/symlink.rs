use anyhow::{Context, Result, bail};
use std::path::Path;

/// Replace a path with a symlink to target.
/// If path is an existing symlink, removes it first.
/// If path is a real directory, errors out (safety).
pub fn replace_with_symlink(path: &Path, target: &Path) -> Result<()> {
    if path.exists() || is_symlink(path) {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            std::fs::remove_file(path)?;
        } else if meta.is_dir() {
            bail!(
                "{} is a real directory. Please move it first (e.g., into a persona skill-set) before switching.\n\
                 Hint: cc-persona snap <name> can capture current config.",
                path.display()
            );
        } else {
            // Regular file (e.g., CLAUDE.md)
            std::fs::remove_file(path)?;
        }
    }

    std::os::unix::fs::symlink(target, path)?;
    Ok(())
}

/// Whether `path` is a symbolic link (does not follow the link).
/// Returns false if the path does not exist or cannot be stat'd.
pub fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Ensure `path` is a real directory.
/// - If it is a symlink: remove the link and create a real directory in its place.
/// - If it is already a real directory: no-op.
/// - If it is a regular file: error out (safety).
pub fn ensure_real_dir(path: &Path) -> Result<()> {
    if is_symlink(path) {
        std::fs::remove_file(path)
            .with_context(|| format!("Failed to remove symlink at {}", path.display()))?;
        std::fs::create_dir_all(path)
            .with_context(|| format!("Failed to create directory at {}", path.display()))?;
        return Ok(());
    }
    if path.is_dir() {
        return Ok(());
    }
    if path.exists() {
        bail!(
            "{} is a regular file, expected a directory. Please move it aside first.",
            path.display()
        );
    }
    std::fs::create_dir_all(path)
        .with_context(|| format!("Failed to create directory at {}", path.display()))?;
    Ok(())
}

/// Link a store skill directory into `~/.claude/skills` via a directory-level symlink.
/// - Only creates the link when `link` does not yet exist.
/// - If `link` is already a real directory, errors out so the caller can treat the
///   skill as shadowed by a wild/untracked directory of the same name.
#[cfg(unix)]
pub fn link_skill(store_skill: &Path, link: &Path) -> Result<()> {
    if is_symlink(link) {
        // Already a symlink — leave it to the caller's reconcile logic.
        return Ok(());
    }
    if link.is_dir() {
        bail!(
            "{} is a real directory; refusing to overwrite (shadowed).",
            link.display()
        );
    }
    if link.exists() {
        bail!(
            "{} already exists and is not a directory; refusing to link.",
            link.display()
        );
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent of {}", link.display()))?;
    }
    std::os::unix::fs::symlink(store_skill, link).with_context(|| {
        format!(
            "Failed to link {} -> {}",
            link.display(),
            store_skill.display()
        )
    })?;
    Ok(())
}

/// Link a store skill directory into `~/.claude/skills`.
/// On Windows, attempts a directory symlink and falls back to a recursive copy
/// when symlink creation fails (e.g. insufficient privileges).
#[cfg(windows)]
pub fn link_skill(store_skill: &Path, link: &Path) -> Result<()> {
    if is_symlink(link) {
        return Ok(());
    }
    if link.is_dir() {
        bail!(
            "{} is a real directory; refusing to overwrite (shadowed).",
            link.display()
        );
    }
    if link.exists() {
        bail!(
            "{} already exists and is not a directory; refusing to link.",
            link.display()
        );
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent of {}", link.display()))?;
    }
    match std::os::windows::fs::symlink_dir(store_skill, link) {
        Ok(()) => Ok(()),
        Err(_) => {
            // TODO: symlink_dir requires Developer Mode or admin on Windows.
            // Fall back to a recursive copy. This breaks the "single shared copy"
            // invariant (I5) on Windows; the manifest should record mode:copy so
            // backup/restore and doctor can account for it.
            copy_dir_recursive(store_skill, link).with_context(|| {
                format!(
                    "Failed to link or copy {} -> {}",
                    store_skill.display(),
                    link.display()
                )
            })
        }
    }
}

#[cfg(windows)]
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;

    #[cfg(unix)]
    #[test]
    fn replace_with_symlink_replaces_files_and_existing_symlinks() {
        let env = TestEnv::new();
        let path = env.paths.root.join("link");
        let first_target = env.paths.root.join("first");
        let second_target = env.paths.root.join("second");

        std::fs::create_dir_all(&first_target).unwrap();
        std::fs::create_dir_all(&second_target).unwrap();
        env.write_file(&path, "old file");

        replace_with_symlink(&path, &first_target).unwrap();
        assert_eq!(std::fs::read_link(&path).unwrap(), first_target);

        replace_with_symlink(&path, &second_target).unwrap();
        assert_eq!(std::fs::read_link(&path).unwrap(), second_target);
    }

    #[test]
    fn replace_with_symlink_errors_for_real_directory() {
        let env = TestEnv::new();
        let path = env.paths.root.join("real-dir");
        let target = env.paths.root.join("target");

        std::fs::create_dir_all(&path).unwrap();
        std::fs::create_dir_all(&target).unwrap();

        let err = replace_with_symlink(&path, &target).unwrap_err();
        assert!(format!("{err:#}").contains("real directory"));
    }
}
