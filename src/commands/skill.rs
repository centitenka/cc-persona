use anyhow::{Result, bail};
use dialoguer::{Confirm, theme::ColorfulTheme};

use crate::active_persona;
use crate::claude::skills;
use crate::config::{AppConfig, Paths};
use crate::persona::{self, Persona};

pub fn run_list(paths: &Paths) -> Result<()> {
    let entries = skills::list_skills_ext(&paths.claude_skills)?;

    if entries.is_empty() {
        eprintln!("No skills found under ~/.claude/skills.");
        return Ok(());
    }

    for entry in &entries {
        let status = if entry.disabled { "OFF" } else { "ON " };
        let kind = if entry.managed {
            "(managed)"
        } else {
            "(untracked)"
        };
        println!("  [{}] {} {}", status, entry.name, kind);
    }

    Ok(())
}

pub fn run_toggle(paths: &Paths, name: &str) -> Result<()> {
    // The link under ~/.claude/skills resolves (through the symlink) to the store
    // SKILL.md, so toggling here writes the single shared copy.
    let skill_md = paths.claude_skills.join(name).join("SKILL.md");
    if !skill_md.exists() {
        bail!("Skill '{}' not found under ~/.claude/skills.", name);
    }

    let now_disabled = skills::toggle_skill(&skill_md)?;
    let status = if now_disabled { "OFF" } else { "ON" };
    eprintln!("✓ Skill '{}' is now {}", name, status);
    eprintln!(
        "  ⚠ This edits the shared store copy and affects every persona that references '{}'.",
        name
    );
    Ok(())
}

pub fn run_rm(paths: &Paths, name: &str) -> Result<()> {
    let link = paths.claude_skills.join(name);
    let store_skill = paths.skill_store.join(name);
    if !link.exists() && !crate::symlink::is_symlink(&link) && !store_skill.exists() {
        bail!("Skill '{}' not found.", name);
    }

    // Detach the managed link under ~/.claude/skills (untracked real dirs untouched).
    skills::unlink_skill(&link)?;

    // Remove the name from the active persona's active list, if present.
    let config = AppConfig::load(&paths.config)?;
    if let Some(active) = config.active_persona.as_deref() {
        let persona_file = persona::persona_path(&paths.personas, active);
        if persona_file.exists() {
            let mut persona = Persona::load(&persona_file)?;
            if let Some(skills_cfg) = persona.skills.as_mut() {
                let before = skills_cfg.active.len();
                skills_cfg.active.retain(|s| s != name);
                if skills_cfg.active.len() != before {
                    persona.save(&persona_file)?;
                    // Keep the dirty-guard snapshot honest after the active-set change.
                    active_persona::write_snapshot(paths, active)?;
                }
            }
        }
    }

    // Offer to delete the shared store copy (default: keep — it's shared, I5).
    if store_skill.exists() {
        let delete_store = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "Also delete '{}' from the shared store? This removes it for ALL personas",
                name
            ))
            .default(false)
            .interact()
            .unwrap_or(false);
        if delete_store {
            skills::remove_from_store(&store_skill)?;
            eprintln!("✓ Skill '{}' removed from the store (all personas)", name);
        } else {
            eprintln!("✓ Skill '{}' unlinked; store copy kept", name);
        }
    } else {
        eprintln!("✓ Skill '{}' unlinked", name);
    }

    Ok(())
}
