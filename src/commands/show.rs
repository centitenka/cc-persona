use anyhow::{Result, bail};

use crate::config::{AppConfig, Paths};
use crate::persona::Persona;

pub fn run(paths: &Paths, name: Option<String>) -> Result<()> {
    let persona_name = match name {
        Some(n) => n,
        None => {
            let config = AppConfig::load(&paths.config)?;
            match config.active_persona {
                Some(n) => n,
                None => bail!("No active persona. Specify a name: cc-persona show <name>"),
            }
        }
    };

    let resolved = Persona::resolve(&persona_name, &paths.personas)?;
    let toml_str = toml::to_string_pretty(&resolved)?;
    println!("{}", toml_str);
    Ok(())
}
