use crate::active_persona;
use anyhow::Result;

use crate::backup;
use crate::config::{AppConfig, Paths};

pub fn run(paths: &Paths, save_current: bool, discard_current: bool) -> Result<()> {
    let config = AppConfig::load(&paths.config)?;
    if config.active_persona.is_none() {
        eprintln!("No active persona. Nothing to restore.");
        return Ok(());
    }

    let persist_choice = active_persona::persist_choice(save_current, discard_current);
    active_persona::guard_and_handle_dirty(paths, persist_choice, &rerun_command(persist_choice))?;

    backup::restore_latest(paths)?;

    // Clear active persona
    let mut config = config;
    config.active_persona = None;
    config.save(&paths.config)?;
    active_persona::clear_snapshot(paths)?;

    eprintln!("✓ Restored original configuration");
    eprintln!("  Active persona: (none)");
    Ok(())
}

fn rerun_command(choice: Option<active_persona::PersistChoice>) -> String {
    let mut command = String::from("cc-persona off");
    match choice {
        Some(active_persona::PersistChoice::Save) => command.push_str(" --save-current"),
        Some(active_persona::PersistChoice::Discard) => command.push_str(" --discard-current"),
        None => {}
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::active_persona;
    use crate::backup;
    use crate::persona;
    use crate::test_support::TestEnv;

    #[cfg(unix)]
    #[test]
    fn run_blocks_when_current_persona_is_dirty_without_explicit_choice() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();

        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n",
        );
        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        env.write_file(&env.paths.claude_settings, "{}");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");
        env.write_file(&env.paths.claude_md_file, "original");
        std::fs::create_dir_all(env.paths.skill_sets.join("engineer")).unwrap();
        crate::claude::skills::switch_skills_symlink(&env.paths, "engineer").unwrap();
        backup::create_backup(&env.paths).unwrap();
        active_persona::write_snapshot(&env.paths, "engineer").unwrap();
        env.write_file(&env.paths.claude_md_file, "dirty");

        let err = run(&env.paths, false, false).unwrap_err();

        assert!(format!("{err:#}").contains("--save-current"));
        assert_eq!(
            AppConfig::load(&env.paths.config)
                .unwrap()
                .active_persona
                .as_deref(),
            Some("engineer")
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_can_save_current_persona_then_restore_original_state() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();

        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n",
        );
        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        env.write_file(&env.paths.claude_settings, "{\"mode\":\"before\"}");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");
        env.write_file(&env.paths.claude_md_file, "before");
        std::fs::create_dir_all(env.paths.skill_sets.join("engineer")).unwrap();
        crate::claude::skills::switch_skills_symlink(&env.paths, "engineer").unwrap();
        backup::create_backup(&env.paths).unwrap();

        env.write_file(&env.paths.claude_settings, "{\"mode\":\"after\"}");
        env.write_file(&env.paths.claude_md_file, "after");
        active_persona::write_snapshot(&env.paths, "engineer").unwrap();
        env.write_file(&env.paths.claude_md_file, "dirty");

        run(&env.paths, true, false).unwrap();

        let saved = persona::Persona::load(&persona::persona_path(&env.paths.personas, "engineer"))
            .unwrap();
        assert_eq!(saved.settings, Some(serde_json::json!({"mode":"after"})));
        assert_eq!(
            env.read_file(&env.paths.claude_md.join("engineer.md")),
            "dirty"
        );
        assert_eq!(
            env.read_file(&env.paths.claude_settings),
            "{\"mode\":\"before\"}"
        );
        assert_eq!(env.read_file(&env.paths.claude_md_file), "before");
        assert!(
            AppConfig::load(&env.paths.config)
                .unwrap()
                .active_persona
                .is_none()
        );
        assert!(!env.paths.active_persona_state.exists());
    }
}
