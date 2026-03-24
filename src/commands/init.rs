use anyhow::{Context, Result};
use std::path::Path;

use crate::config::Paths;

pub const SKILL_CONTENT: &str = r#"---
name: cc-persona
description: |
  Switch Claude Code persona/profile configuration. Use when the user mentions
  "switch persona", "change profile", "切换人格", "切换角色", "用工程师模式",
  "switch to engineer", "switch to designer", "plain mode", "切换到",
  or wants to change their Claude Code working mode/role.
allowed-tools: [Bash]
---

# CC Persona — Claude Code Configuration Switcher

When the user wants to switch their Claude Code persona/working mode, use
the `cc-persona` CLI tool via Bash.

## Available Commands

- `cc-persona list` — List all available personas (marks active one)
- `cc-persona use <name>` — Switch to a persona
- `cc-persona which` — Show current active persona
- `cc-persona off` — Restore original configuration
- `cc-persona skill list` — List skills and their status in current persona
- `cc-persona skill toggle <name>` — Toggle a skill on/off
- `cc-persona show [name]` — Show full resolved config of a persona
- `cc-persona diff [name]` — Compare current config with a persona

## Workflow

1. If user wants to see options: run `cc-persona list`
2. If user names a persona: run `cc-persona use <name>`
3. If user wants to revert: run `cc-persona off`
4. After switching, inform the user what changed
5. **ALWAYS** remind the user to restart their Claude Code session after switching

## CRITICAL: Post-switch reminder

After ANY successful persona switch (`use` or `off`), you MUST tell the user:

> Persona 已切换。由于 skills、MCP 和 settings 在会话启动时加载，
> 请输入 `/exit` 退出后重新启动 Claude Code 以使所有变更完全生效。

This is mandatory — never skip this reminder. The switch modifies config files
on disk, but the current Claude Code session still holds the old state in memory.
"#;

/// Install cc-persona SKILL.md into a given directory.
pub fn install_skill(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("SKILL.md"), SKILL_CONTENT)
        .context("Failed to write cc-persona SKILL.md")?;
    Ok(())
}

/// Ensure cc-persona skill exists in every skill-set directory.
pub fn ensure_skill_in_all_skill_sets(paths: &Paths) -> Result<()> {
    if !paths.skill_sets.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&paths.skill_sets)? {
        let entry = entry?;
        if entry.path().is_dir() {
            let target = entry.path().join("cc-persona");
            // Always overwrite to ensure latest version
            install_skill(&target)?;
        }
    }
    Ok(())
}

pub fn run(paths: &Paths) -> Result<()> {
    // Create cc-persona directories
    paths.ensure_dirs()?;
    eprintln!("✓ Created ~/.cc-persona/ directory structure");

    // Create default plain persona if none exists
    let plain_path = paths.personas.join("plain.toml");
    if !plain_path.exists() {
        let default_plain = r#"name = "plain"
description = "Minimal default persona — clean slate"
"#;
        std::fs::write(&plain_path, default_plain)?;
        eprintln!("✓ Created default 'plain' persona");
    }

    // Create plain skill-set directory
    let plain_skills = paths.skill_sets.join("plain");
    if !plain_skills.exists() {
        std::fs::create_dir_all(&plain_skills)?;
    }

    // Create plain claude-md
    let plain_md = paths.claude_md.join("plain.md");
    if !plain_md.exists() {
        std::fs::write(&plain_md, "")?;
    }

    // Install cc-persona skill into ALL existing skill-sets
    ensure_skill_in_all_skill_sets(paths)?;

    // Also install directly into ~/.claude/skills/cc-persona/ if it's a real dir
    // (before first persona switch, skills is still a real directory)
    if !paths.claude_skills.is_symlink() && paths.claude_skills.is_dir() {
        install_skill(&paths.claude_skills.join("cc-persona"))?;
    }

    eprintln!("✓ Installed cc-persona skill for Claude Code");
    eprintln!();
    eprintln!("cc-persona is ready! Next steps:");
    eprintln!("  cc-persona create <name>   — Create a persona");
    eprintln!("  cc-persona snap <name>     — Snapshot current config as a persona");
    eprintln!("  cc-persona list            — See available personas");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;

    #[test]
    fn run_creates_default_layout_and_installs_skill() {
        let env = TestEnv::new();
        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();

        run(&env.paths).unwrap();

        assert!(env.paths.root.exists());
        assert!(env.paths.personas.join("plain.toml").exists());
        assert!(
            env.paths
                .skill_sets
                .join("plain")
                .join("cc-persona")
                .join("SKILL.md")
                .exists()
        );
        assert!(env.paths.claude_md.join("plain.md").exists());
        assert!(
            env.paths
                .claude_skills
                .join("cc-persona")
                .join("SKILL.md")
                .exists()
        );
        assert!(
            env.read_file(&env.paths.personas.join("plain.toml"))
                .contains("name = \"plain\"")
        );
    }
}
