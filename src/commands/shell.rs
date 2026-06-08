use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::process::Command;

use crate::active_persona;
use crate::commands::use_cmd;
use crate::config::{Paths, Target};
use crate::persona::{self, Persona};

/// EXPERIMENTAL window scope: materialize a persona into an isolated
/// `CLAUDE_CONFIG_DIR` and launch Claude Code against it, so two windows — even on
/// the same project — can hold different personas at once.
///
/// This depends on the undocumented `CLAUDE_CONFIG_DIR` and its `.claude.json`
/// placement, which may change across Claude Code versions.
pub fn run(paths: &Paths, persona_name: &str) -> Result<()> {
    let persona_file = persona::persona_path(&paths.personas, persona_name);
    if !persona_file.exists() {
        bail!(
            "Persona '{}' not found. Use `cc-persona list` to see available personas.",
            persona_name
        );
    }
    let resolved = Persona::resolve(persona_name, &paths.personas)
        .with_context(|| format!("Failed to resolve persona '{}'", persona_name))?;

    // Allocate an isolated config dir under ~/.cc-persona/windows/<id>/.
    let id = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let window_root = paths.root.join("windows").join(&id);
    let config_dir = window_root.join("claude");
    std::fs::create_dir_all(&config_dir)?;

    // A fully isolated target: every Claude destination lives inside config_dir.
    let target = Target {
        settings_file: config_dir.join("settings.json"),
        skills_dir: config_dir.join("skills"),
        claude_md_file: Some(config_dir.join("CLAUDE.md")),
        claude_json: config_dir.join(".claude.json"),
        claude_json_project_key: None,
        snapshot_path: window_root.join("active-persona-state.json"),
        backups_dir: window_root.join("backups"),
        include_cc_persona_skill: true,
    };

    use_cmd::apply_persona(paths, &target, &resolved, persona_name)?;
    active_persona::write_snapshot(&target, persona_name)?;

    eprintln!();
    eprintln!("🧪 Experimental window scope (relies on the undocumented CLAUDE_CONFIG_DIR).");
    eprintln!("   Isolated config dir: {}", config_dir.display());

    match find_claude() {
        Some(bin) => {
            eprintln!("   Launching claude with CLAUDE_CONFIG_DIR set…\n");
            let status = Command::new(&bin)
                .env("CLAUDE_CONFIG_DIR", &config_dir)
                .status()
                .with_context(|| format!("Failed to launch {}", bin.display()))?;
            std::process::exit(status.code().unwrap_or(0));
        }
        None => {
            eprintln!("   `claude` was not found on PATH. Start it yourself with:");
            println!("export CLAUDE_CONFIG_DIR={}", config_dir.display());
            Ok(())
        }
    }
}

/// Best-effort `claude` lookup on `PATH`.
fn find_claude() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("claude");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
