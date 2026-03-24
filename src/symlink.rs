use anyhow::{Result, bail};
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

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
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
