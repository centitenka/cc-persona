use anyhow::{Result, bail};

use crate::claude::skills;
use crate::config::Paths;

pub fn run_list(paths: &Paths) -> Result<()> {
    let skills_dir = resolve_skills_dir(paths)?;
    let skill_list = skills::list_skills(&skills_dir)?;

    if skill_list.is_empty() {
        eprintln!("No skills found in current skill-set.");
        return Ok(());
    }

    for (name, disabled) in &skill_list {
        let status = if *disabled { "OFF" } else { "ON " };
        println!("  [{}] {}", status, name);
    }

    Ok(())
}

pub fn run_toggle(paths: &Paths, name: &str) -> Result<()> {
    let skills_dir = resolve_skills_dir(paths)?;
    let skill_md = skills_dir.join(name).join("SKILL.md");
    if !skill_md.exists() {
        bail!("Skill '{}' not found.", name);
    }

    let now_disabled = skills::toggle_skill(&skill_md)?;
    let status = if now_disabled { "OFF" } else { "ON" };
    eprintln!("✓ Skill '{}' is now {}", name, status);
    Ok(())
}

pub fn run_rm(paths: &Paths, name: &str) -> Result<()> {
    let skills_dir = resolve_skills_dir(paths)?;
    let skill_dir = skills_dir.join(name);
    if !skill_dir.exists() {
        bail!("Skill '{}' not found.", name);
    }

    skills::remove_skill(&skills_dir, name)?;
    eprintln!("✓ Skill '{}' removed permanently", name);
    Ok(())
}

fn resolve_skills_dir(paths: &Paths) -> Result<std::path::PathBuf> {
    if paths.claude_skills.is_symlink() {
        Ok(std::fs::read_link(&paths.claude_skills)?)
    } else {
        Ok(paths.claude_skills.clone())
    }
}
