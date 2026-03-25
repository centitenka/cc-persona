use anyhow::{Context, Result, bail};
use dialoguer::{Select, theme::ColorfulTheme};

use crate::active_persona::{self, PersistChoice};
use crate::backup;
use crate::claude::{claude_md, mcp, settings, skills};
use crate::commands::init;
use crate::config::{AppConfig, Paths};
use crate::persona::{self, Persona};

pub fn run(
    paths: &Paths,
    name: Option<String>,
    save_current: bool,
    discard_current: bool,
) -> Result<()> {
    let persona_name = match name {
        Some(n) => n,
        None => interactive_select(paths)?,
    };

    // Verify persona exists
    let persona_file = persona::persona_path(&paths.personas, &persona_name);
    if !persona_file.exists() {
        bail!(
            "Persona '{}' not found. Use `cc-persona list` to see available personas.",
            persona_name
        );
    }

    // Resolve persona (with inheritance)
    let resolved = Persona::resolve(&persona_name, &paths.personas)
        .with_context(|| format!("Failed to resolve persona '{}'", persona_name))?;

    let persist_choice = active_persona::persist_choice(save_current, discard_current);
    active_persona::guard_and_handle_dirty(
        paths,
        persist_choice,
        &rerun_command(&persona_name, persist_choice),
    )?;

    // Backup current state
    let backup_dir = backup::create_backup(paths)?;
    eprintln!("✓ Backed up current config to {}", backup_dir.display());

    // Apply settings.json overrides
    if let Some(ref overrides) = resolved.settings {
        let current = settings::read_settings(&paths.claude_settings)?;
        let merged = settings::apply_overrides(&current, overrides);
        settings::write_settings(&paths.claude_settings, &merged)?;
        eprintln!("✓ Applied settings.json overrides");
    }

    // Switch skills symlink
    skills::switch_skills_symlink(paths, &persona_name)?;
    // Ensure cc-persona skill is always present in the target skill-set
    let target_skill_set = paths.skill_sets.join(&persona_name);
    init::install_skill(&target_skill_set.join("cc-persona"))?;
    eprintln!("✓ Switched skills → {}", persona_name);

    // Apply per-skill toggles
    if let Some(ref skills_config) = resolved.skills {
        let skill_set_dir = paths.skill_sets.join(&persona_name);
        skills::apply_skill_toggles(&skill_set_dir, skills_config)?;
        eprintln!("✓ Applied skill toggles");
    }

    // Apply MCP config
    if let Some(ref mcp_config) = resolved.mcp {
        let mut claude_json = mcp::read_claude_json(&paths.claude_json)?;
        mcp::apply_mcp_config(&mut claude_json, mcp_config)?;
        mcp::write_claude_json(&paths.claude_json, &claude_json)?;
        eprintln!("✓ Applied MCP server toggles");
    }

    // Switch CLAUDE.md
    if let Some(ref md_config) = resolved.claude_md {
        claude_md::switch_claude_md(paths, md_config)?;
        eprintln!("✓ Switched CLAUDE.md");
    }

    // Update active persona in config
    let mut config = AppConfig::load(&paths.config)?;
    config.active_persona = Some(persona_name.clone());
    config.save(&paths.config)?;
    active_persona::write_snapshot(paths, &persona_name)?;

    eprintln!("\n🎭 Switched to persona: {}", persona_name);
    Ok(())
}

fn rerun_command(persona_name: &str, persist_choice: Option<PersistChoice>) -> String {
    let mut command = format!("cc-persona use {}", persona_name);
    match persist_choice {
        Some(PersistChoice::Save) => command.push_str(" --save-current"),
        Some(PersistChoice::Discard) => command.push_str(" --discard-current"),
        None => {}
    }
    command
}

fn interactive_select(paths: &Paths) -> Result<String> {
    let names = persona::list_personas(&paths.personas)?;
    if names.is_empty() {
        bail!("No personas found. Create one with: cc-persona create <name>");
    }

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select persona")
        .items(&names)
        .default(0)
        .interact()
        .context("Selection cancelled")?;

    Ok(names[selection].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::active_persona;
    use crate::test_support::TestEnv;
    use serde_json::json;

    #[cfg(unix)]
    #[test]
    fn run_switches_persona_and_updates_filesystem_state() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();

        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            r#"
name = "engineer"
description = "Engineer mode"

[settings]
model = "claude-opus"
newFlag = true

[settings.ui]
theme = "dark"

[skills]
active = ["alpha"]

[mcp]
enable = ["GitHub"]
disable = ["Figma"]

[claude_md]
file = "engineer.md"
"#,
        );

        env.write_file(
            &env.paths.claude_settings,
            &json!({
                "model": "claude-sonnet",
                "ui": {
                    "theme": "light",
                    "font": "mono"
                },
                "features": {
                    "safe": true
                }
            })
            .to_string(),
        );
        env.write_file(
            &env.paths.claude_json,
            &json!({
                "mcpServers": {
                    "GitHub Prod": {
                        "command": "github",
                        "disabled": true
                    },
                    "Figma Design": {
                        "command": "figma"
                    },
                    "Linear": {
                        "command": "linear",
                        "disabled": true
                    }
                }
            })
            .to_string(),
        );
        env.write_file(&env.paths.claude_md_file, "current session instructions");
        env.write_file(
            &env.paths.claude_md.join("engineer.md"),
            "engineer instructions",
        );

        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();
        env.create_skill(&env.paths.claude_skills, "alpha", "---\nname: alpha\n---\n");
        env.create_skill(&env.paths.claude_skills, "beta", "---\nname: beta\n---\n");

        run(&env.paths, Some("engineer".to_string()), false, false).unwrap();

        let backup_entries: Vec<_> = std::fs::read_dir(&env.paths.backups)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(backup_entries.len(), 1);
        let backup_dir = &backup_entries[0];
        assert!(backup_dir.join("settings.json").exists());
        assert!(backup_dir.join("CLAUDE.md").exists());

        let settings_value = settings::read_settings(&env.paths.claude_settings).unwrap();
        assert_eq!(
            settings_value,
            json!({
                "model": "claude-opus",
                "ui": {
                    "theme": "dark",
                    "font": "mono"
                },
                "features": {
                    "safe": true
                },
                "newFlag": true
            })
        );

        assert!(env.paths.claude_skills.is_symlink());
        assert_eq!(
            std::fs::read_link(&env.paths.claude_skills).unwrap(),
            env.paths.skill_sets.join("engineer")
        );
        assert!(
            env.paths
                .skill_sets
                .join("engineer")
                .join("cc-persona")
                .join("SKILL.md")
                .exists()
        );
        assert!(
            !skills::list_skills(&env.paths.skill_sets.join("engineer"))
                .unwrap()
                .into_iter()
                .find(|(name, _)| name == "alpha")
                .unwrap()
                .1
        );
        assert!(
            skills::list_skills(&env.paths.skill_sets.join("engineer"))
                .unwrap()
                .into_iter()
                .find(|(name, _)| name == "beta")
                .unwrap()
                .1
        );

        let claude_json = mcp::read_claude_json(&env.paths.claude_json).unwrap();
        let servers = claude_json["mcpServers"].as_object().unwrap();
        assert!(servers["GitHub Prod"].get("disabled").is_none());
        assert_eq!(servers["Figma Design"]["disabled"], json!(true));
        assert_eq!(servers["Linear"]["disabled"], json!(true));

        assert!(env.paths.claude_md_file.is_symlink());
        assert_eq!(
            std::fs::read_link(&env.paths.claude_md_file).unwrap(),
            env.paths.claude_md.join("engineer.md")
        );

        let config = AppConfig::load(&env.paths.config).unwrap();
        assert_eq!(config.active_persona.as_deref(), Some("engineer"));
        assert!(env.paths.active_persona_state.exists());
    }

    #[cfg(unix)]
    #[test]
    fn run_blocks_when_current_persona_is_dirty_without_explicit_choice() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();

        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n",
        );
        env.write_file(
            &persona::persona_path(&env.paths.personas, "designer"),
            "name = \"designer\"\n",
        );
        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        env.write_file(&env.paths.claude_settings, "{}");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");
        env.write_file(&env.paths.claude_md_file, "clean");
        std::fs::create_dir_all(env.paths.skill_sets.join("engineer")).unwrap();
        std::fs::create_dir_all(env.paths.skill_sets.join("designer")).unwrap();
        skills::switch_skills_symlink(&env.paths, "engineer").unwrap();

        active_persona::write_snapshot(&env.paths, "engineer").unwrap();
        env.write_file(&env.paths.claude_md_file, "dirty");

        let err = run(&env.paths, Some("designer".to_string()), false, false).unwrap_err();

        assert!(format!("{err:#}").contains("unsaved changes"));
        assert_eq!(
            AppConfig::load(&env.paths.config)
                .unwrap()
                .active_persona
                .as_deref(),
            Some("engineer")
        );
        assert_eq!(std::fs::read_dir(&env.paths.backups).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn run_can_discard_dirty_current_persona_and_continue() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();

        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n",
        );
        env.write_file(
            &persona::persona_path(&env.paths.personas, "designer"),
            "name = \"designer\"\n",
        );
        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        env.write_file(&env.paths.claude_settings, "{}");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");
        env.write_file(&env.paths.claude_md_file, "clean");
        std::fs::create_dir_all(env.paths.skill_sets.join("engineer")).unwrap();
        std::fs::create_dir_all(env.paths.skill_sets.join("designer")).unwrap();
        skills::switch_skills_symlink(&env.paths, "engineer").unwrap();

        active_persona::write_snapshot(&env.paths, "engineer").unwrap();
        env.write_file(&env.paths.claude_md_file, "dirty");

        run(&env.paths, Some("designer".to_string()), false, true).unwrap();

        assert_eq!(
            AppConfig::load(&env.paths.config)
                .unwrap()
                .active_persona
                .as_deref(),
            Some("designer")
        );
        assert!(env.paths.active_persona_state.exists());
    }
}
