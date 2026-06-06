use anyhow::{Result, bail};

use crate::claude::{mcp, settings};
use crate::config::Paths;
use crate::diagnostics;
use crate::persona::{self, ClaudeMdConfig, McpConfig, Persona, SkillsConfig};

pub fn run(paths: &Paths, name: Option<String>) -> Result<()> {
    let persona_name = name.unwrap_or_else(|| {
        chrono::Local::now()
            .format("snap-%Y%m%d-%H%M%S")
            .to_string()
    });

    let persona_file = persona::persona_path(&paths.personas, &persona_name);
    if persona_file.exists() {
        bail!("Persona '{}' already exists.", persona_name);
    }

    // Snapshot settings.json
    let current_settings = settings::read_settings(&paths.claude_settings)?;

    // Snapshot skills: active = the set of managed per-skill links currently under
    // ~/.claude/skills (symlinks into the shared store). Wild (real) subdirectories
    // are NOT captured — they belong to no persona until adopted. Warn so the user
    // can `cc-persona adopt` them first if they want them tracked.
    let managed = diagnostics::list_managed_links(paths)?;
    let mut active_skills: Vec<String> = managed.into_iter().map(|(name, _)| name).collect();
    active_skills.sort();

    let untracked = diagnostics::list_untracked_skills(paths)?;
    if !untracked.is_empty() {
        eprintln!(
            "  ⚠ {} untracked skill(s) not captured (wild directories): {}",
            untracked.len(),
            untracked.join(", ")
        );
        eprintln!("    Run `cc-persona adopt` first if you want them in this persona.");
    }

    // Snapshot MCP
    let mcp_servers = mcp::list_mcp_servers(&paths.claude_json)?;
    let enabled_mcp: Vec<String> = mcp_servers
        .iter()
        .filter(|(_, disabled)| !disabled)
        .map(|(name, _)| name.clone())
        .collect();
    let disabled_mcp: Vec<String> = mcp_servers
        .iter()
        .filter(|(_, disabled)| *disabled)
        .map(|(name, _)| name.clone())
        .collect();

    // Create persona
    let persona = Persona {
        name: persona_name.clone(),
        description: format!(
            "Snapshot taken at {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M")
        ),
        base: None,
        settings: Some(current_settings),
        skills: Some(SkillsConfig {
            active: active_skills,
        }),
        mcp: Some(McpConfig {
            enable: enabled_mcp,
            disable: disabled_mcp,
        }),
        claude_md: Some(ClaudeMdConfig {
            file: Some(format!("{}.md", persona_name)),
        }),
    };

    paths.ensure_dirs()?;
    persona.save(&persona_file)?;

    // Skills are shared via the store — no physical copy is made. The persona only
    // references active skill names; activation happens via per-skill links built
    // on `cc-persona use`.

    // Copy current CLAUDE.md
    let md_target = paths.claude_md.join(format!("{}.md", persona_name));
    if paths.claude_md_file.exists() {
        // Resolve symlink to get actual content
        let content = std::fs::read_to_string(&paths.claude_md_file).unwrap_or_default();
        std::fs::write(&md_target, content)?;
    } else {
        std::fs::write(&md_target, "")?;
    }

    eprintln!("✓ Snapped current config as persona '{}'", persona_name);
    eprintln!("  File: {}", persona_file.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;
    use serde_json::json;

    #[cfg(unix)]
    #[test]
    fn run_captures_current_configuration_into_persona_assets() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();

        env.write_file(
            &env.paths.claude_settings,
            &json!({
                "model": "claude-sonnet",
                "ui": {
                    "theme": "light"
                }
            })
            .to_string(),
        );
        env.write_file(
            &env.paths.claude_json,
            &json!({
                "mcpServers": {
                    "GitHub": {
                        "command": "github"
                    },
                    "Figma": {
                        "command": "figma",
                        "disabled": true
                    }
                }
            })
            .to_string(),
        );
        env.write_file(&env.paths.claude_md_file, "current claude instructions");

        // New model: ~/.claude/skills is a real directory; managed links point into
        // the shared store. `alpha` is managed (active); `wild` is untracked and
        // must not be captured.
        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.link_into_claude_skills("alpha");
        env.create_skill(&env.paths.claude_skills, "wild", "---\nname: wild\n---\n");

        run(&env.paths, Some("engineer".to_string())).unwrap();

        let persona_file = persona::persona_path(&env.paths.personas, "engineer");
        assert!(persona_file.exists());

        let snapped = Persona::load(&persona_file).unwrap();
        assert_eq!(snapped.name, "engineer");
        assert_eq!(
            snapped.settings,
            Some(json!({
                "model": "claude-sonnet",
                "ui": {
                    "theme": "light"
                }
            }))
        );
        // Only the managed link `alpha` is captured; `wild` is excluded.
        assert_eq!(snapped.skills.unwrap().active, vec!["alpha"]);
        let mcp = snapped.mcp.unwrap();
        assert_eq!(mcp.enable, vec!["GitHub"]);
        assert_eq!(mcp.disable, vec!["Figma"]);
        assert_eq!(
            snapped.claude_md.unwrap().file.as_deref(),
            Some("engineer.md")
        );
        assert_eq!(
            env.read_file(&env.paths.claude_md.join("engineer.md")),
            "current claude instructions"
        );
    }
}
