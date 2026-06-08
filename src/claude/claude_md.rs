use anyhow::{Context, Result};
use std::path::Path;

use crate::persona::ClaudeMdConfig;
use crate::symlink::replace_with_symlink;

/// Switch `md_target_file` (the live CLAUDE.md) to point at the persona's stored
/// md file (under `store_md_dir`) via a symlink. Used only at scopes that manage
/// CLAUDE.md (global/window); project scope leaves CLAUDE.md to the user level.
pub fn switch_claude_md(
    md_target_file: &Path,
    store_md_dir: &Path,
    config: &ClaudeMdConfig,
) -> Result<()> {
    let filename = match &config.file {
        Some(f) => f.as_str(),
        None => return Ok(()), // No claude_md config, skip
    };

    let target = store_md_dir.join(filename);
    if !target.exists() {
        // Create an empty file so the symlink target exists
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, "")
            .with_context(|| format!("Failed to create {}", target.display()))?;
    }

    replace_with_symlink(md_target_file, &target).context("Failed to switch CLAUDE.md symlink")?;
    Ok(())
}
