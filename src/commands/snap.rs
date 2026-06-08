use anyhow::{Result, bail};

use crate::claude::{mcp, settings, skills};
use crate::config::{Paths, Scope, Target};
use crate::persona::{self, ClaudeMdConfig, McpConfig, Persona, SkillsConfig};

pub fn run(paths: &Paths, scope: &Scope, name: Option<String>) -> Result<()> {
    let persona_name = name.unwrap_or_else(|| {
        chrono::Local::now()
            .format("snap-%Y%m%d-%H%M%S")
            .to_string()
    });

    let persona_file = persona::persona_path(&paths.personas, &persona_name);
    if persona_file.exists() {
        bail!("Persona '{}' already exists.", persona_name);
    }

    let target = paths.resolve_target(scope);

    // Snapshot settings (settings.json at global, settings.local.json at project).
    let current_settings = settings::read_settings(&target.settings_file)?;

    // Snapshot skills: active = the managed per-skill links currently under the
    // scope's skills dir (symlinks into the shared store). Wild (real)
    // subdirectories are NOT captured — they belong to no persona until adopted.
    let entries = skills::list_skills_ext(&target.skills_dir)?;
    let mut active_skills: Vec<String> = entries
        .iter()
        .filter(|e| e.managed)
        .map(|e| e.name.clone())
        .collect();
    active_skills.sort();

    let untracked: Vec<String> = entries
        .iter()
        .filter(|e| !e.managed && e.name != "cc-persona")
        .map(|e| e.name.clone())
        .collect();
    if !untracked.is_empty() {
        eprintln!(
            "  ⚠ {} untracked skill(s) not captured (wild directories): {}",
            untracked.len(),
            untracked.join(", ")
        );
        eprintln!("    Run `cc-persona adopt` first if you want them in this persona.");
    }

    // Snapshot MCP.
    let (enabled_mcp, disabled_mcp) = snapshot_mcp(&target)?;

    // Create persona. CLAUDE.md is only captured at scopes that manage it.
    let claude_md = target
        .claude_md_file
        .as_ref()
        .map(|_| ClaudeMdConfig {
            file: Some(format!("{}.md", persona_name)),
        });

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
        claude_md,
    };

    paths.ensure_dirs()?;
    persona.save(&persona_file)?;

    // Copy current CLAUDE.md (managed scopes only).
    if let Some(md_file) = &target.claude_md_file {
        let md_target = paths.claude_md.join(format!("{}.md", persona_name));
        let content = if md_file.exists() {
            std::fs::read_to_string(md_file).unwrap_or_default()
        } else {
            String::new()
        };
        std::fs::write(&md_target, content)?;
    }

    eprintln!("✓ Snapped current config as persona '{}'", persona_name);
    eprintln!("  File: {}", persona_file.display());
    Ok(())
}

/// Capture `(enable, disable)` MCP name lists for the scope: top-level servers at
/// global, the connector disabled list at project scope.
fn snapshot_mcp(target: &Target) -> Result<(Vec<String>, Vec<String>)> {
    match &target.claude_json_project_key {
        None => {
            let servers = mcp::list_mcp_servers(&target.claude_json)?;
            let enabled = servers
                .iter()
                .filter(|(_, disabled)| !disabled)
                .map(|(name, _)| name.clone())
                .collect();
            let disabled = servers
                .iter()
                .filter(|(_, disabled)| *disabled)
                .map(|(name, _)| name.clone())
                .collect();
            Ok((enabled, disabled))
        }
        Some(key) => {
            let disabled = mcp::list_disabled_connectors(&target.claude_json, key)?;
            Ok((Vec::new(), disabled))
        }
    }
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

        run(&env.paths, &Scope::Global, Some("engineer".to_string())).unwrap();

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
