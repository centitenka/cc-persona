use anyhow::Result;

use crate::config::{AppConfig, Paths};
use crate::diagnostics;
use crate::persona::{self, Persona};

pub fn run(paths: &Paths) -> Result<()> {
    let config = AppConfig::load(&paths.config)?;
    let names = persona::list_personas(&paths.personas)?;

    if names.is_empty() {
        eprintln!("No personas found. Create one with: cc-persona create <name>");
        return Ok(());
    }

    for name in &names {
        let active = config.active_persona.as_ref().is_some_and(|a| a == name);
        let marker = if active { " *" } else { "" };

        // Try to load description
        let desc = match Persona::load(&paths.personas.join(format!("{}.toml", name))) {
            Ok(p) if !p.description.is_empty() => format!(" — {}", p.description),
            _ => String::new(),
        };

        println!("  {}{}{}", name, marker, desc);
    }

    if let Some(active) = &config.active_persona {
        eprintln!("\n  (* = active: {})", active);
    }

    print_drift_hint(paths, config.active_persona.as_deref());
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
