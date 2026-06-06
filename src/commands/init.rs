use anyhow::{Context, Result};
use std::path::Path;

use crate::config::Paths;
use crate::symlink;

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
- `cc-persona use <name> --save-current` — Save current persona changes, then switch
- `cc-persona use <name> --discard-current` — Discard current persona changes, then switch
- `cc-persona which` — Show current active persona
- `cc-persona off` — Restore original configuration
- `cc-persona off --save-current` — Save current persona changes, then restore
- `cc-persona off --discard-current` — Discard current persona changes, then restore
- `cc-persona skill list` — List skills and their status in current persona
- `cc-persona skill toggle <name>` — Toggle a skill on/off
- `cc-persona show [name]` — Show full resolved config of a persona
- `cc-persona diff [name]` — Compare current config with a persona
- `cc-persona doctor` — Health-check skills/plugins/MCP state and report drift (alias: `status`)
- `cc-persona adopt [names...]` — Take untracked skills under management (add to a persona)
- `cc-persona migrate` — Migrate a v0.1 layout to the shared skill-store + per-skill links

## Workflow

1. If user wants to see options: run `cc-persona list`
2. If user names a persona: run `cc-persona use <name>`
3. If user wants to revert: run `cc-persona off`
4. If `use`/`off` reports unsaved changes in the current persona, ask whether to save or discard them
5. Recommend saving by default, then rerun with `--save-current` or `--discard-current`
6. After switching, inform the user what changed
7. **ALWAYS** remind the user to run `/reload-skills` (and `/reload-plugins` if plugins changed) after switching; MCP/settings changes may still need a session restart

## Dirty persona guard

If the current persona has unsaved changes, `cc-persona use` and `cc-persona off`
will stop and require an explicit choice:

- Use `--save-current` to persist the current persona first (recommended)
- Use `--discard-current` to continue without saving

Never silently choose `--discard-current`. Ask the user first.

## CRITICAL: Post-switch reminder

After ANY successful persona switch (`use` or `off`), you MUST tell the user:

> Persona 已切换。运行 `/reload-skills` 让 skill 变更生效；若本次改动了插件，再运行 `/reload-plugins`。MCP 或 settings 改动可能仍需 `/exit` 重启 Claude Code。

This is mandatory — never skip this reminder. The switch modifies config files
on disk, but the current Claude Code session still holds the old state in memory.
Reloading (or, for MCP/settings, restarting) is what makes Claude pick up the change.
"#;

/// Install cc-persona SKILL.md into a given directory.
pub fn install_skill(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("SKILL.md"), SKILL_CONTENT)
        .context("Failed to write cc-persona SKILL.md")?;
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

    // Install the cc-persona skill as the single shared copy in the store.
    let store_cc_persona = paths.skill_store.join("cc-persona");
    install_skill(&store_cc_persona)?;

    // If ~/.claude/skills is still a legacy whole-directory symlink (v0.1 model),
    // we cannot safely manage per-skill links yet — prompt the user to migrate.
    if symlink::is_symlink(&paths.claude_skills) {
        eprintln!(
            "  ⚠ ~/.claude/skills is a symlink (legacy v0.1 model). Run `cc-persona migrate` to switch to the shared store + per-skill links."
        );
    } else {
        // ~/.claude/skills is (or becomes) a real directory; link cc-persona in.
        symlink::ensure_real_dir(&paths.claude_skills)?;
        let cc_link = paths.claude_skills.join("cc-persona");
        if !symlink::is_symlink(&cc_link) && !cc_link.exists() {
            symlink::link_skill(&store_cc_persona, &cc_link)?;
        }
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

    #[cfg(unix)]
    #[test]
    fn run_creates_default_layout_and_installs_skill() {
        let env = TestEnv::new();
        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();

        run(&env.paths).unwrap();

        assert!(env.paths.root.exists());
        assert!(env.paths.personas.join("plain.toml").exists());
        // cc-persona is now installed as the single shared store copy.
        assert!(
            env.paths
                .skill_store
                .join("cc-persona")
                .join("SKILL.md")
                .exists()
        );
        assert!(env.paths.claude_md.join("plain.md").exists());
        // ~/.claude/skills stays a real directory; cc-persona is a managed link.
        assert!(!env.paths.claude_skills.is_symlink());
        assert!(env.paths.claude_skills.join("cc-persona").is_symlink());
        // The link resolves to the store SKILL.md.
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
        let skill = env.read_file(&env.paths.claude_skills.join("cc-persona").join("SKILL.md"));
        assert!(skill.contains("--save-current"));
        assert!(skill.contains("--discard-current"));
    }
}
