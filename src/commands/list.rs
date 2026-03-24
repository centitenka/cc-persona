use anyhow::Result;

use crate::config::{AppConfig, Paths};
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

    Ok(())
}
