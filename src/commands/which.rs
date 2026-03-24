use anyhow::Result;

use crate::config::{AppConfig, Paths};

pub fn run(paths: &Paths) -> Result<()> {
    let config = AppConfig::load(&paths.config)?;
    match config.active_persona {
        Some(name) => println!("{}", name),
        None => println!("(none)"),
    }
    Ok(())
}
