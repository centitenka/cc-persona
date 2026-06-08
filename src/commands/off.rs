use crate::active_persona;
use anyhow::Result;

use crate::backup;
use crate::config::{AppConfig, Paths, Scope};

pub fn run(paths: &Paths, scope: &Scope, save_current: bool, discard_current: bool) -> Result<()> {
    let config = AppConfig::load(&paths.config)?;
    if config.binding(scope).is_none() {
        eprintln!("No active persona for this scope. Nothing to restore.");
        return Ok(());
    }

    let target = paths.resolve_target(scope);
    let persist_choice = active_persona::persist_choice(save_current, discard_current);
    active_persona::guard_and_handle_dirty(
        paths,
        &target,
        scope,
        persist_choice,
        &rerun_command(scope, persist_choice),
    )?;

    let lock = paths.root.join("claude-json.lock");
    backup::restore_latest(&target, &paths.skill_store, &lock)?;

    // Clear the scope's binding.
    let mut config = config;
    config.set_binding(scope, None);
    config.save(&paths.config)?;
    active_persona::clear_snapshot(&target)?;

    eprintln!("✓ Restored original configuration");
    eprintln!("  Active persona: (none)");
    Ok(())
}

fn rerun_command(scope: &Scope, choice: Option<active_persona::PersistChoice>) -> String {
    let mut command = String::from("cc-persona off");
    if !scope.is_global() {
        command.push_str(" --project");
    }
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
        backup::create_backup(&env.global_target(), &env.paths.skill_store).unwrap();
        active_persona::write_snapshot(&env.global_target(), "engineer").unwrap();
        env.write_file(&env.paths.claude_md_file, "dirty");

        let err = run(&env.paths, &Scope::Global, false, false).unwrap_err();

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
        backup::create_backup(&env.global_target(), &env.paths.skill_store).unwrap();

        env.write_file(&env.paths.claude_settings, "{\"mode\":\"after\"}");
        env.write_file(&env.paths.claude_md_file, "after");
        active_persona::write_snapshot(&env.global_target(), "engineer").unwrap();
        env.write_file(&env.paths.claude_md_file, "dirty");

        run(&env.paths, &Scope::Global, true, false).unwrap();

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

    #[cfg(unix)]
    #[test]
    fn run_project_scope_deletes_created_targets_and_clears_binding() {
        use crate::commands::use_cmd;

        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n\n[settings]\nmodel = \"opus\"\n\n[skills]\nactive = [\"alpha\"]\n",
        );
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");

        let cwd = env.project_cwd("api");
        let proj = Scope::Project(cwd.clone());

        // First switch creates <cwd>/.claude/settings.local.json + skills/alpha link
        // (neither existed before, so the backup records them as cc-persona-created).
        use_cmd::run(&env.paths, &proj, Some("engineer".to_string()), false, false).unwrap();
        let local_settings = cwd.join(".claude").join("settings.local.json");
        let proj_skills = cwd.join(".claude").join("skills");
        assert!(local_settings.exists());
        assert!(proj_skills.join("alpha").is_symlink());
        assert_eq!(
            AppConfig::load(&env.paths.config).unwrap().binding(&proj),
            Some("engineer")
        );

        run(&env.paths, &proj, false, false).unwrap();

        // Delete-on-restore removed exactly what cc-persona created.
        assert!(!local_settings.exists());
        assert!(!proj_skills.exists());
        // Binding + per-project snapshot cleared.
        let config = AppConfig::load(&env.paths.config).unwrap();
        assert_eq!(config.binding(&proj), None);
        assert!(!env.target(&proj).snapshot_path.exists());
    }
}
