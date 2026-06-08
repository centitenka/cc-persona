use anyhow::{Context, Result};
use std::path::Path;

use crate::active_persona;
use crate::backup;
use crate::claude::skills;
use crate::commands::init;
use crate::config::{AppConfig, Paths};
use crate::persona::Persona;
use crate::symlink;

const CC_PERSONA_SKILL: &str = "cc-persona";

/// Migrate a v0.1 layout (whole-directory skills symlink + per-persona skill-sets)
/// onto the v0.2 model (shared store + per-skill links under a real `~/.claude/skills`).
///
/// Idempotent. Steps:
/// 1. Backup current Claude Code config.
/// 2. Ingest every `skill-sets/*/<skill>` into the shared store (move, or copy with `--copy`).
/// 3. Install the cc-persona skill into the store.
/// 4. Restore `~/.claude/skills` to a real directory if it was a symlink.
/// 5. If a persona is active, reconcile its links and re-snapshot the live state.
pub fn run(paths: &Paths, copy: bool) -> Result<()> {
    paths.ensure_dirs()?;

    // (0) Backup first — migrate is destructive in `move` mode.
    let backup_dir = backup::create_backup(&paths.global_target(), &paths.skill_store)?;
    eprintln!("✓ Backed up current config to {}", backup_dir.display());

    // (A) Pull every skill-set skill into the shared store.
    let ingested = ingest_skill_sets_into_store(paths, copy)?;
    eprintln!(
        "✓ Ingested {} skill(s) into the store ({} mode)",
        ingested,
        if copy { "copy" } else { "move" }
    );

    // (B) Ensure the cc-persona skill exists as a single store copy.
    init::install_skill(&paths.skill_store.join(CC_PERSONA_SKILL))?;

    // (C) Restore ~/.claude/skills to a real directory.
    let was_symlink = symlink::is_symlink(&paths.claude_skills);
    symlink::ensure_real_dir(&paths.claude_skills)?;
    if was_symlink {
        eprintln!("✓ Restored ~/.claude/skills to a real directory");
    }

    // (D) If a persona is active, rebuild its per-skill links from the store.
    let config = AppConfig::load(&paths.config)?;
    if let Some(active) = config.active_persona.as_deref() {
        let resolved = Persona::resolve(active, &paths.personas)
            .with_context(|| format!("Failed to resolve active persona '{}'", active))?;
        let report = skills::reconcile_skills(
            &paths.claude_skills,
            &paths.skill_store,
            &resolved,
            true,
        )?;
        eprintln!(
            "✓ Reconciled skills for '{}' ({} linked)",
            active,
            report.linked.len()
        );
        // (E) Repair the dirty-guard snapshot.
        active_persona::write_snapshot(&paths.global_target(), active)?;
    }

    eprintln!(
        "✓ Migration complete. Run /reload-skills (and /reload-plugins if plugins changed) to apply."
    );
    Ok(())
}

/// Pull every `skill-sets/*/<skill>` directory into the shared store.
///
/// Cross-set name collisions keep the first copy (the store is single-source, I5).
/// `cc-persona` is skipped (installed fresh from `SKILL_CONTENT`). Each ingested
/// SKILL.md has any stale `disable-model-invocation` flag cleared. Returns the
/// number of skills newly placed in the store.
pub fn ingest_skill_sets_into_store(paths: &Paths, copy: bool) -> Result<usize> {
    if !paths.skill_sets.exists() {
        return Ok(0);
    }
    std::fs::create_dir_all(&paths.skill_store)?;

    let mut ingested = 0usize;
    for set_entry in std::fs::read_dir(&paths.skill_sets)? {
        let set_entry = set_entry?;
        let set_path = set_entry.path();
        if !set_path.is_dir() {
            continue;
        }

        for skill_entry in std::fs::read_dir(&set_path)? {
            let skill_entry = skill_entry?;
            let skill_path = skill_entry.path();
            if !skill_path.is_dir() {
                continue;
            }
            let name = skill_entry.file_name().to_string_lossy().to_string();
            if name == CC_PERSONA_SKILL {
                continue;
            }

            let dest = paths.skill_store.join(&name);
            if dest.exists() {
                // Already in the store — first copy wins (I5).
                continue;
            }

            if copy {
                copy_dir_recursive(&skill_path, &dest)?;
            } else {
                move_dir(&skill_path, &dest)?;
            }
            skills::clear_disable_flag(&dest.join("SKILL.md"))?;
            ingested += 1;
        }
    }
    Ok(ingested)
}

/// Move a directory, falling back to recursive copy + remove across filesystems.
fn move_dir(src: &Path, dst: &Path) -> Result<()> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_dir_recursive(src, dst)?;
            std::fs::remove_dir_all(src)
                .with_context(|| format!("Failed to remove {} after copy", src.display()))?;
            Ok(())
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;

    #[cfg(unix)]
    #[test]
    fn run_replays_legacy_layout_into_store_and_links() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();

        // Legacy: skill-sets/engineer holds the physical skills; ~/.claude/skills
        // is a whole-directory symlink to it. active = [alpha].
        let set = env.paths.skill_sets.join("engineer");
        std::fs::create_dir_all(&set).unwrap();
        env.create_skill(&set, "alpha", "---\nname: alpha\n---\n");
        env.create_skill(
            &set,
            "beta",
            "---\nname: beta\ndisable-model-invocation: true\n---\n",
        );
        env.create_skill(&set, "cc-persona", "---\nname: cc-persona\n---\n");
        env.symlink(&set, &env.paths.claude_skills);

        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        env.write_file(
            &crate::persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n\n[skills]\nactive = [\"alpha\"]\n",
        );
        env.write_file(&env.paths.claude_settings, "{}");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");

        run(&env.paths, false).unwrap();

        // Store has alpha + beta + cc-persona.
        assert!(env.paths.skill_store.join("alpha").exists());
        assert!(env.paths.skill_store.join("beta").exists());
        assert!(env.paths.skill_store.join("cc-persona").exists());
        // beta's stale disable flag cleared on ingest.
        assert!(
            !env.read_file(&env.paths.skill_store.join("beta").join("SKILL.md"))
                .contains("disable-model-invocation")
        );
        // ~/.claude/skills is now a real directory.
        assert!(!env.paths.claude_skills.is_symlink());
        // Only alpha (active) + cc-persona are linked.
        assert!(env.paths.claude_skills.join("alpha").is_symlink());
        assert!(env.paths.claude_skills.join("cc-persona").is_symlink());
        assert!(!env.paths.claude_skills.join("beta").exists());
        // Snapshot written.
        assert!(env.paths.active_persona_state.exists());
    }

    #[cfg(unix)]
    #[test]
    fn run_is_idempotent() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        let set = env.paths.skill_sets.join("engineer");
        std::fs::create_dir_all(&set).unwrap();
        env.create_skill(&set, "alpha", "---\nname: alpha\n---\n");
        env.symlink(&set, &env.paths.claude_skills);
        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        env.write_file(
            &crate::persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n\n[skills]\nactive = [\"alpha\"]\n",
        );
        env.write_file(&env.paths.claude_settings, "{}");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");

        run(&env.paths, false).unwrap();
        run(&env.paths, false).unwrap();

        assert!(env.paths.skill_store.join("alpha").exists());
        assert!(env.paths.claude_skills.join("alpha").is_symlink());
    }
}
