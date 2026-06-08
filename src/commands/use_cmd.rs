use anyhow::{Context, Result, bail};
use dialoguer::{Select, theme::ColorfulTheme};
use std::path::Path;

use crate::active_persona::{self, PersistChoice};
use crate::backup;
use crate::claude::{claude_md, mcp, settings, skills};
use crate::config::{AppConfig, Paths, ProjectMeta, Scope, Target};
use crate::persona::{self, Persona};
use crate::symlink;

pub fn run(
    paths: &Paths,
    scope: &Scope,
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

    let target = paths.resolve_target(scope);

    let persist_choice = active_persona::persist_choice(save_current, discard_current);
    active_persona::guard_and_handle_dirty(
        paths,
        &target,
        scope,
        persist_choice,
        &rerun_command(scope, &persona_name, persist_choice),
    )?;

    apply_persona(paths, &target, &resolved, &persona_name)?;

    // Project scope keeps cc-persona state out of the repo and gitignores the
    // machine-local targets it writes.
    if let Scope::Project(cwd) = scope {
        write_project_meta(paths, cwd, &persona_name)?;
        ensure_project_gitignore(cwd);
    }

    // Update the scope's binding + dirty snapshot.
    let mut config = AppConfig::load(&paths.config)?;
    config.set_binding(scope, Some(persona_name.clone()));
    config.save(&paths.config)?;
    active_persona::write_snapshot(&target, &persona_name)?;

    eprintln!("\n🎭 Switched to persona: {} {}", persona_name, scope_label(scope));
    Ok(())
}

/// Apply a resolved persona to `target`: backup, then settings + skills + MCP
/// (servers always; connectors at project scope) + CLAUDE.md (managed scopes
/// only). Shared by `use` and the experimental `shell` launcher.
pub fn apply_persona(
    paths: &Paths,
    target: &Target,
    resolved: &Persona,
    persona_name: &str,
) -> Result<()> {
    // Backup current state
    let backup_dir = backup::create_backup(target, &paths.skill_store)?;
    eprintln!("✓ Backed up current config to {}", backup_dir.display());

    // Apply settings overrides
    if let Some(ref overrides) = resolved.settings {
        let current = settings::read_settings(&target.settings_file)?;
        let merged = settings::apply_overrides(&current, overrides);
        settings::write_settings(&target.settings_file, &merged)?;
        eprintln!("✓ Applied settings overrides");
    }

    // Reconcile skills: the skills dir is a real directory holding per-skill
    // symlinks into the shared store. cc-persona is force-linked only at scopes
    // that own the user-level skills dir.
    symlink::ensure_real_dir(&target.skills_dir)?;
    let report = skills::reconcile_skills(
        &target.skills_dir,
        &paths.skill_store,
        resolved,
        target.include_cc_persona_skill,
    )?;
    eprintln!(
        "✓ Reconciled skills → {} ({} linked, {} unlinked)",
        persona_name,
        report.linked.len(),
        report.unlinked.len()
    );
    if !report.untracked.is_empty() {
        eprintln!(
            "  ⚠ {} untracked skill(s) not managed by cc-persona. Run `cc-persona adopt` to take them over (details: `cc-persona doctor`).",
            report.untracked.len()
        );
    }

    // Apply MCP config: top-level servers at every scope, plus connectors under
    // projects.<cwd> at project scope. One locked, atomic read-modify-write.
    if let Some(ref mcp_config) = resolved.mcp {
        let lock = paths.root.join("claude-json.lock");
        let project_key = target.claude_json_project_key.clone();
        mcp::update_claude_json(&target.claude_json, &lock, |json| {
            mcp::apply_mcp_servers(json, mcp_config)?;
            if let Some(cwd_key) = &project_key {
                // apply_connectors reconciles cwd_key to Claude Code's literal
                // projects.<key> itself, matching every read/restore path.
                mcp::apply_connectors(json, cwd_key, mcp_config)?;
            }
            Ok(())
        })?;
        eprintln!("✓ Applied MCP server toggles");
    }

    // Switch CLAUDE.md — only at scopes that manage it (global/window).
    if let (Some(md_file), Some(md_config)) = (&target.claude_md_file, &resolved.claude_md) {
        claude_md::switch_claude_md(md_file, &paths.claude_md, md_config)?;
        eprintln!("✓ Switched CLAUDE.md");
    }

    Ok(())
}

fn scope_label(scope: &Scope) -> String {
    match scope {
        Scope::Global => "(global)".to_string(),
        Scope::Project(cwd) => format!("(project: {})", cwd.display()),
    }
}

/// Write `~/.cc-persona/projects/<enc>/meta.json` describing this project binding.
fn write_project_meta(paths: &Paths, cwd: &Path, persona_name: &str) -> Result<()> {
    let state_root = paths.project_state_root(cwd);
    std::fs::create_dir_all(&state_root)?;
    let meta_path = state_root.join("meta.json");
    let now = chrono::Local::now().to_rfc3339();
    let created = std::fs::read_to_string(&meta_path)
        .ok()
        .and_then(|c| serde_json::from_str::<ProjectMeta>(&c).ok())
        .and_then(|m| m.created)
        .or_else(|| Some(now.clone()));
    let meta = ProjectMeta {
        project_path: cwd.to_string_lossy().into_owned(),
        created,
        last_used: Some(now),
    };
    std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
    let _ = persona_name; // bound via AppConfig.projects; meta is for enumeration
    Ok(())
}

/// Ensure `<cwd>/.claude/.gitignore` ignores the machine-local files cc-persona
/// writes (they embed `$HOME` paths and should never be committed). Best-effort.
fn ensure_project_gitignore(cwd: &Path) {
    let claude_dir = cwd.join(".claude");
    if std::fs::create_dir_all(&claude_dir).is_err() {
        return;
    }
    let gitignore = claude_dir.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
    let mut additions = String::new();
    for entry in ["settings.local.json", "skills/"] {
        if !existing.lines().any(|l| l.trim() == entry) {
            additions.push_str(entry);
            additions.push('\n');
        }
    }
    if additions.is_empty() {
        return;
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&additions);
    let _ = std::fs::write(&gitignore, content);
}

fn rerun_command(
    scope: &Scope,
    persona_name: &str,
    persist_choice: Option<PersistChoice>,
) -> String {
    let mut command = format!("cc-persona use {}", persona_name);
    if !scope.is_global() {
        command.push_str(" --project");
    }
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

        // New model: ~/.claude/skills is a real directory; the shared store holds
        // the single copies; reconcile links exactly the desired set.
        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.create_store_skill("beta", "---\nname: beta\n---\n");
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");

        run(
            &env.paths,
            &Scope::Global,
            Some("engineer".to_string()),
            false,
            false,
        )
        .unwrap();

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

        // ~/.claude/skills stays a real directory (I1).
        assert!(!env.paths.claude_skills.is_symlink());
        // alpha (active) is a per-skill symlink into the store.
        let alpha_link = env.paths.claude_skills.join("alpha");
        assert!(
            skills::list_skills_ext(&env.paths.claude_skills)
                .unwrap()
                .iter()
                .any(|e| e.name == "alpha" && e.managed)
        );
        assert_eq!(
            std::fs::read_link(&alpha_link).unwrap(),
            env.paths.skill_store.join("alpha")
        );
        // beta is not active → no link.
        assert!(!env.paths.claude_skills.join("beta").exists());
        // cc-persona is always linked.
        assert!(env.paths.claude_skills.join("cc-persona").is_symlink());

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

        active_persona::write_snapshot(&env.global_target(), "engineer").unwrap();
        env.write_file(&env.paths.claude_md_file, "dirty");

        let err = run(
            &env.paths,
            &Scope::Global,
            Some("designer".to_string()),
            false,
            false,
        )
        .unwrap_err();

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

        active_persona::write_snapshot(&env.global_target(), "engineer").unwrap();
        env.write_file(&env.paths.claude_md_file, "dirty");

        run(
            &env.paths,
            &Scope::Global,
            Some("designer".to_string()),
            false,
            true,
        )
        .unwrap();

        assert_eq!(
            AppConfig::load(&env.paths.config)
                .unwrap()
                .active_persona
                .as_deref(),
            Some("designer")
        );
        assert!(env.paths.active_persona_state.exists());
    }

    #[cfg(unix)]
    #[test]
    fn run_project_scope_writes_local_targets_and_leaves_global_untouched() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();

        // The persona also declares [claude_md] — to prove project scope ignores it.
        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n\n[settings]\nmodel = \"claude-opus\"\n\n[skills]\nactive = [\"alpha\"]\n\n[claude_md]\nfile = \"engineer.md\"\n",
        );
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");

        // Global user-level state that must stay byte-for-byte untouched.
        env.write_file(&env.paths.claude_settings, "{\"global\":true}");
        env.write_file(&env.paths.claude_md_file, "user-level CLAUDE.md");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");

        let cwd = env.project_cwd("api");
        let scope = Scope::Project(cwd.clone());

        run(&env.paths, &scope, Some("engineer".to_string()), false, false).unwrap();

        // Project targets written under <cwd>/.claude/.
        let local_settings = cwd.join(".claude").join("settings.local.json");
        assert!(local_settings.exists());
        assert_eq!(
            settings::read_settings(&local_settings).unwrap()["model"],
            json!("claude-opus")
        );

        // alpha is linked into the PROJECT skills dir; cc-persona is NOT force-linked
        // at project scope (the user-level link already provides it).
        let proj_skills = cwd.join(".claude").join("skills");
        assert!(proj_skills.join("alpha").is_symlink());
        assert!(!proj_skills.join("cc-persona").exists());

        // The machine-local targets are gitignored on first use.
        let gitignore = env.read_file(&cwd.join(".claude").join(".gitignore"));
        assert!(gitignore.contains("settings.local.json"));
        assert!(gitignore.contains("skills/"));

        // Binding recorded at project scope; the global binding is untouched.
        let config = AppConfig::load(&env.paths.config).unwrap();
        assert_eq!(config.binding(&scope), Some("engineer"));
        assert_eq!(config.active_persona, None);

        // Global user-level files are untouched — including CLAUDE.md, which project
        // scope never manages (it is a user-level dimension only).
        assert_eq!(env.read_file(&env.paths.claude_settings), "{\"global\":true}");
        assert!(!env.paths.claude_md_file.is_symlink());
        assert_eq!(
            env.read_file(&env.paths.claude_md_file),
            "user-level CLAUDE.md"
        );

        // Per-project state lives outside the repo, with a meta.json + its own snapshot.
        assert!(
            env.paths
                .project_state_root(&cwd)
                .join("meta.json")
                .exists()
        );
        assert!(env.target(&scope).snapshot_path.exists());
        assert!(!env.paths.active_persona_state.exists());
    }

    #[cfg(unix)]
    #[test]
    fn two_project_scopes_hold_different_personas_independently() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n\n[skills]\nactive = [\"alpha\"]\n",
        );
        env.write_file(
            &persona::persona_path(&env.paths.personas, "designer"),
            "name = \"designer\"\n\n[skills]\nactive = [\"beta\"]\n",
        );
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.create_store_skill("beta", "---\nname: beta\n---\n");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");

        let api = env.project_cwd("api");
        let web = env.project_cwd("web");
        run(
            &env.paths,
            &Scope::Project(api.clone()),
            Some("engineer".to_string()),
            false,
            false,
        )
        .unwrap();
        run(
            &env.paths,
            &Scope::Project(web.clone()),
            Some("designer".to_string()),
            false,
            false,
        )
        .unwrap();

        let config = AppConfig::load(&env.paths.config).unwrap();
        assert_eq!(config.binding(&Scope::Project(api.clone())), Some("engineer"));
        assert_eq!(config.binding(&Scope::Project(web.clone())), Some("designer"));
        assert_eq!(config.active_persona, None);

        // Each project links only its own active skill.
        assert!(api.join(".claude/skills/alpha").is_symlink());
        assert!(!api.join(".claude/skills/beta").exists());
        assert!(web.join(".claude/skills/beta").is_symlink());
        assert!(!web.join(".claude/skills/alpha").exists());
    }

    #[cfg(unix)]
    #[test]
    fn dirty_global_persona_does_not_block_a_project_switch() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n",
        );
        env.write_file(
            &persona::persona_path(&env.paths.personas, "designer"),
            "name = \"designer\"\n\n[skills]\nactive = [\"beta\"]\n",
        );
        env.create_store_skill("beta", "---\nname: beta\n---\n");
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");
        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        env.write_file(&env.paths.claude_settings, "{}");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");
        env.write_file(&env.paths.claude_md_file, "clean");
        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();

        // Snapshot global as engineer, then dirty the global state.
        active_persona::write_snapshot(&env.global_target(), "engineer").unwrap();
        env.write_file(&env.paths.claude_md_file, "dirty");

        // A project switch must proceed despite the dirty GLOBAL persona, because the
        // dirty guard is keyed on the scope's own binding (the project has none yet).
        let cwd = env.project_cwd("api");
        let scope = Scope::Project(cwd.clone());
        run(&env.paths, &scope, Some("designer".to_string()), false, false).unwrap();

        let config = AppConfig::load(&env.paths.config).unwrap();
        assert_eq!(config.binding(&scope), Some("designer"));
        // Global binding unchanged.
        assert_eq!(config.active_persona.as_deref(), Some("engineer"));
    }

    #[cfg(unix)]
    #[test]
    fn dirty_project_persona_does_not_block_a_global_switch() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n",
        );
        env.write_file(
            &persona::persona_path(&env.paths.personas, "designer"),
            "name = \"designer\"\n\n[skills]\nactive = [\"alpha\"]\n",
        );
        env.write_file(
            &persona::persona_path(&env.paths.personas, "plain"),
            "name = \"plain\"\n",
        );
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");
        env.write_file(&env.paths.claude_settings, "{}");
        env.write_file(&env.paths.claude_md_file, "clean");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");
        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();

        // A clean global active persona.
        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        active_persona::write_snapshot(&env.global_target(), "engineer").unwrap();

        // A project bound to designer, then made dirty with a stray managed link.
        let cwd = env.project_cwd("api");
        let proj = Scope::Project(cwd.clone());
        run(&env.paths, &proj, Some("designer".to_string()), false, false).unwrap();
        env.create_store_skill("stray", "---\nname: stray\n---\n");
        env.symlink(
            &env.paths.skill_store.join("stray"),
            &env.target(&proj).skills_dir.join("stray"),
        );

        // The global switch proceeds; the dirty project is irrelevant to it.
        run(
            &env.paths,
            &Scope::Global,
            Some("plain".to_string()),
            false,
            false,
        )
        .unwrap();

        let config = AppConfig::load(&env.paths.config).unwrap();
        assert_eq!(config.active_persona.as_deref(), Some("plain"));
        assert_eq!(config.binding(&proj), Some("designer"));
    }
}
