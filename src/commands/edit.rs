use anyhow::{Result, bail};
use std::process::Command;

use crate::config::Paths;
use crate::persona;

pub fn run(paths: &Paths, name: &str) -> Result<()> {
    let persona_file = persona::persona_path(&paths.personas, name);
    if !persona_file.exists() {
        bail!("Persona '{}' not found.", name);
    }

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = Command::new(&editor).arg(&persona_file).status()?;

    if !status.success() {
        bail!("Editor exited with error");
    }

    // Validate the file is still valid TOML
    match persona::Persona::load(&persona_file) {
        Ok(_) => eprintln!("✓ Persona '{}' saved and validated", name),
        Err(e) => eprintln!("⚠ Warning: persona file has errors: {}", e),
    }

    Ok(())
}
