use anyhow::{Context, Result, bail};
use dialoguer::{MultiSelect, theme::ColorfulTheme};

use crate::active_persona;
use crate::claude::skills;
use crate::config::{AppConfig, Paths};
use crate::diagnostics;
use crate::persona::{self, Persona, SkillsConfig};

/// Adopt untracked skills (wild real directories under `~/.claude/skills`) into a
/// persona: move each into the shared store and add it to the target persona's
/// `[skills].active` list. Reconciles + re-snapshots when the target is active.
pub fn run(paths: &Paths, into: Option<String>, names: Vec<String>) -> Result<()> {
    let config = AppConfig::load(&paths.config)?;
    let target = match into.or_else(|| config.active_persona.clone()) {
        Some(t) => t,
        None => bail!(
            "No target persona. Pass --into <persona> or activate one with `cc-persona use <persona>`."
        ),
    };

    let target_file = persona::persona_path(&paths.personas, &target);
    if !target_file.exists() {
        bail!(
            "Persona '{}' not found. Use `cc-persona list` to see available personas.",
            target
        );
    }

    let untracked = diagnostics::list_untracked_skills(paths)?;
    if untracked.is_empty() {
        eprintln!("No untracked skills to adopt.");
        return Ok(());
    }

    // Decide which untracked skills to adopt.
    let selected: Vec<String> = if names.is_empty() {
        select_interactive(&untracked)?
    } else {
        for name in &names {
            if !untracked.iter().any(|u| u == name) {
                bail!(
                    "Skill '{}' is not an untracked skill under ~/.claude/skills.",
                    name
                );
            }
        }
        names
    };

    if selected.is_empty() {
        eprintln!("Nothing selected. No skills adopted.");
        return Ok(());
    }

    // Move each selected skill into the store (skip if already present there).
    let mut adopted = Vec::new();
    for name in &selected {
        let store_skill = paths.skill_store.join(name);
        let wild = paths.claude_skills.join(name);
        if store_skill.exists() {
            eprintln!(
                "  ⚠ '{}' already exists in the store; skipping move (left in place under ~/.claude/skills).",
                name
            );
            continue;
        }
        std::fs::create_dir_all(&paths.skill_store)?;
        std::fs::rename(&wild, &store_skill)
            .with_context(|| format!("Failed to move {} into the store", wild.display()))?;
        adopted.push(name.clone());
    }

    // Merge adopted names into the target persona's active list and save.
    let mut persona = Persona::load(&target_file)
        .with_context(|| format!("Failed to load persona '{}'", target))?;
    persona.skills = Some(merge_active(persona.skills.clone(), &selected));
    persona.save(&target_file)?;

    // If the target is the active persona, rebuild links + snapshot so the
    // adoption takes effect immediately and the dirty-guard stays honest.
    if config.active_persona.as_deref() == Some(target.as_str()) {
        let resolved = Persona::resolve(&target, &paths.personas)?;
        skills::reconcile_skills(
            &paths.claude_skills,
            &paths.skill_store,
            &resolved,
            true,
        )?;
        active_persona::write_snapshot(&paths.global_target(), &target)?;
    }

    eprintln!(
        "✓ 已纳管 {} 个 skill,运行 /reload-skills 生效",
        adopted.len()
    );
    Ok(())
}

/// Interactive multi-select over the untracked skill names.
fn select_interactive(untracked: &[String]) -> Result<Vec<String>> {
    let chosen = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select skills to adopt (space to toggle, enter to confirm)")
        .items(untracked)
        .interact()
        .context("Selection cancelled")?;
    Ok(chosen.into_iter().map(|i| untracked[i].clone()).collect())
}

/// Merge `names` into a skills config's active list, deduplicated and sorted.
fn merge_active(existing: Option<SkillsConfig>, names: &[String]) -> SkillsConfig {
    let mut active = existing.map(|s| s.active).unwrap_or_default();
    active.extend(names.iter().cloned());
    active.sort();
    active.dedup();
    SkillsConfig { active }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;

    #[test]
    fn merge_active_dedups_and_sorts() {
        let existing = Some(SkillsConfig {
            active: vec!["beta".to_string(), "alpha".to_string()],
        });
        let merged = merge_active(existing, &["alpha".to_string(), "gamma".to_string()]);
        assert_eq!(
            merged.active,
            vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_adopts_into_active_persona_and_links() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();
        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n",
        );
        env.write_file(&env.paths.claude_settings, "{}");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");
        env.create_skill(&env.paths.claude_skills, "wild", "---\nname: wild\n---\n");
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");

        run(&env.paths, None, vec!["wild".to_string()]).unwrap();

        // Moved into the store.
        assert!(env.paths.skill_store.join("wild").join("SKILL.md").exists());
        // Added to active.
        let p = Persona::load(&persona::persona_path(&env.paths.personas, "engineer")).unwrap();
        assert!(p.skills.unwrap().active.contains(&"wild".to_string()));
        // Linked (active persona).
        assert!(env.paths.claude_skills.join("wild").is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn run_into_non_active_persona_only_edits_toml() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();
        // No active persona set.
        env.write_file(
            &persona::persona_path(&env.paths.personas, "designer"),
            "name = \"designer\"\n",
        );
        env.create_skill(&env.paths.claude_skills, "wild", "---\nname: wild\n---\n");

        run(
            &env.paths,
            Some("designer".to_string()),
            vec!["wild".to_string()],
        )
        .unwrap();

        let p = Persona::load(&persona::persona_path(&env.paths.personas, "designer")).unwrap();
        assert!(p.skills.unwrap().active.contains(&"wild".to_string()));
        // No link created for a non-active target.
        assert!(!env.paths.claude_skills.join("wild").exists());
    }
}
