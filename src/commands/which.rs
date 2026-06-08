use anyhow::Result;

use crate::config::{AppConfig, Paths, Scope};
use crate::diagnostics;
use crate::persona::Persona;

pub fn run(paths: &Paths, scope: &Scope) -> Result<()> {
    let config = AppConfig::load(&paths.config)?;
    let binding = config.binding(scope);
    match binding {
        Some(name) => println!("{}", name),
        None => println!("(none)"),
    }

    // At global scope, surface any project bindings so multi-window users can see
    // which projects hold which personas at a glance.
    if scope.is_global() && !config.projects.is_empty() {
        eprintln!("\n  Project bindings:");
        for (path, b) in &config.projects {
            eprintln!("    {} → {}", path, b.persona);
        }
    }

    print_drift_hint(paths, binding);
    Ok(())
}

/// Read-only reconcile-on-read: surface skill drift (untracked / ghost / missing
/// or extra links / legacy symlink dir) as a single hint line. Never writes.
fn print_drift_hint(paths: &Paths, active_name: Option<&str>) {
    let active = active_name.and_then(|name| Persona::resolve(name, &paths.personas).ok());
    let Ok(drift) = diagnostics::inspect_skills(paths, active.as_ref()) else {
        return;
    };
    if drift.skills_dir_is_symlink {
        eprintln!("  ⚠ ~/.claude/skills is a legacy symlink. Run `cc-persona migrate`.");
        return;
    }
    let has_drift = !drift.untracked.is_empty()
        || !drift.ghosts.is_empty()
        || !drift.drifted_missing_link.is_empty()
        || !drift.drifted_extra_link.is_empty();
    if has_drift {
        eprintln!("  ⚠ Skill drift detected. Run `cc-persona doctor` for details.");
    }
}
