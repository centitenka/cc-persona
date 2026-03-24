use anyhow::Result;

use crate::backup;
use crate::config::{AppConfig, Paths};

pub fn run(paths: &Paths) -> Result<()> {
    let config = AppConfig::load(&paths.config)?;
    if config.active_persona.is_none() {
        eprintln!("No active persona. Nothing to restore.");
        return Ok(());
    }

    backup::restore_latest(paths)?;

    // Clear active persona
    let mut config = config;
    config.active_persona = None;
    config.save(&paths.config)?;

    eprintln!("✓ Restored original configuration");
    eprintln!("  Active persona: (none)");
    Ok(())
}
