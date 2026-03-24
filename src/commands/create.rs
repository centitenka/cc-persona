use anyhow::{Result, bail};
use dialoguer::{Input, theme::ColorfulTheme};

use crate::commands::init;
use crate::config::Paths;
use crate::persona::{self, Persona};

pub fn run(paths: &Paths, name: &str) -> Result<()> {
    let persona_file = persona::persona_path(&paths.personas, name);
    if persona_file.exists() {
        bail!(
            "Persona '{}' already exists. Use `cc-persona edit {}` to modify it.",
            name,
            name
        );
    }

    let description: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Description")
        .default(String::new())
        .interact_text()?;

    let names = persona::list_personas(&paths.personas)?;
    let base = if !names.is_empty() {
        let base_input: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Base persona (empty for none)")
            .default(String::new())
            .interact_text()?;
        if base_input.is_empty() {
            None
        } else {
            Some(base_input)
        }
    } else {
        None
    };

    let persona = Persona {
        name: name.to_string(),
        description,
        base,
        ..Default::default()
    };

    paths.ensure_dirs()?;
    persona.save(&persona_file)?;

    // Create skill-set directory with cc-persona skill
    let skill_set = paths.skill_sets.join(name);
    std::fs::create_dir_all(&skill_set)?;
    init::install_skill(&skill_set.join("cc-persona"))?;

    // Create claude-md placeholder
    let md_file = paths.claude_md.join(format!("{}.md", name));
    if !md_file.exists() {
        std::fs::write(&md_file, "")?;
    }

    eprintln!("✓ Created persona '{}' at {}", name, persona_file.display());
    eprintln!("  Edit with: cc-persona edit {}", name);
    Ok(())
}
