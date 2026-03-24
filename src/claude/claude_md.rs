use anyhow::{Context, Result};

use crate::config::Paths;
use crate::persona::ClaudeMdConfig;
use crate::symlink::replace_with_symlink;

/// Switch CLAUDE.md to point to the persona's md file via symlink.
pub fn switch_claude_md(paths: &Paths, config: &ClaudeMdConfig) -> Result<()> {
    let filename = match &config.file {
        Some(f) => f.as_str(),
        None => return Ok(()), // No claude_md config, skip
    };

    let target = paths.claude_md.join(filename);
    if !target.exists() {
        // Create an empty file so the symlink target exists
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, "")
            .with_context(|| format!("Failed to create {}", target.display()))?;
    }

    replace_with_symlink(&paths.claude_md_file, &target)
        .context("Failed to switch CLAUDE.md symlink")?;
    Ok(())
}
