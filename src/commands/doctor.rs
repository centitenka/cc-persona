use anyhow::{Result, bail};

use crate::claude::mcp;
use crate::config::{AppConfig, Paths};
use crate::diagnostics;
use crate::persona::Persona;

/// Run a read-only health check over the cc-persona / Claude Code state.
///
/// Prints one line per finding prefixed with ✓ (ok), ⚠ (warning) or ✗ (error),
/// each problem carrying a `Fix:` hint. Exits non-zero when any ✗ is present so
/// the command composes in scripts the way the rest of the CLI does on error.
pub fn run(paths: &Paths) -> Result<()> {
    let config = AppConfig::load(&paths.config)?;
    let active_name = config.active_persona.clone();
    let active = match active_name.as_deref() {
        Some(name) => Persona::resolve(name, &paths.personas).ok(),
        None => None,
    };

    let mut errors = 0usize;

    eprintln!("cc-persona doctor");
    eprintln!(
        "  Active persona: {}",
        active_name.as_deref().unwrap_or("(none)")
    );
    eprintln!();

    // --- Skills directory invariant (I1) ---
    let drift = diagnostics::inspect_skills(paths, active.as_ref())?;

    if drift.skills_dir_is_symlink {
        eprintln!("✗ ~/.claude/skills is a symlink (legacy whole-directory model).");
        eprintln!("    Fix: cc-persona migrate");
        errors += 1;
    } else {
        eprintln!("✓ ~/.claude/skills is a real directory (I1).");
    }

    // --- Drift between persona and managed links ---
    if drift.drifted_missing_link.is_empty() && drift.drifted_extra_link.is_empty() {
        eprintln!("✓ Managed skill links match the active persona.");
    } else {
        if !drift.drifted_missing_link.is_empty() {
            eprintln!(
                "✗ {} desired skill(s) missing a link: {}",
                drift.drifted_missing_link.len(),
                drift.drifted_missing_link.join(", ")
            );
            errors += 1;
        }
        if !drift.drifted_extra_link.is_empty() {
            eprintln!(
                "✗ {} stray managed link(s) not desired: {}",
                drift.drifted_extra_link.len(),
                drift.drifted_extra_link.join(", ")
            );
            errors += 1;
        }
        eprintln!(
            "    Fix: cc-persona use {}",
            active_name.as_deref().unwrap_or("<persona>")
        );
    }

    // --- Ghosts (I4) ---
    if drift.ghosts.is_empty() {
        eprintln!("✓ Every active skill exists in the store (I4).");
    } else {
        eprintln!(
            "✗ {} skill(s) referenced but missing from the store: {}",
            drift.ghosts.len(),
            drift.ghosts.join(", ")
        );
        eprintln!(
            "    Fix: cc-persona migrate  (or remove the name from the persona's active list)"
        );
        errors += 1;
    }

    // --- Foreign links ---
    if drift.foreign_links.is_empty() {
        eprintln!("✓ No foreign symlinks under ~/.claude/skills.");
    } else {
        eprintln!(
            "⚠ {} symlink(s) point outside the store: {}",
            drift.foreign_links.len(),
            drift.foreign_links.join(", ")
        );
        eprintln!("    Fix: inspect manually; cc-persona does not manage these.");
    }

    // --- Untracked skills ---
    if drift.untracked.is_empty() {
        eprintln!("✓ No untracked skills.");
    } else {
        eprintln!(
            "⚠ {} untracked skill(s): {}",
            drift.untracked.len(),
            drift.untracked.join(", ")
        );
        eprintln!("    Fix: cc-persona adopt");
    }

    // --- Untracked plugins ---
    let untracked_plugins = diagnostics::untracked_plugins(paths)?;
    if untracked_plugins.is_empty() {
        eprintln!("✓ No untracked plugins.");
    } else {
        eprintln!(
            "⚠ {} enabled plugin(s) not declared in any persona: {}",
            untracked_plugins.len(),
            untracked_plugins.join(", ")
        );
        eprintln!(
            "    Fix: declare them in a persona's [settings].enabledPlugins, or `cc-persona snap`."
        );
    }

    // --- MCP source 1: top-level mcpServers ---
    let untracked_mcp = diagnostics::untracked_mcp(paths)?;
    if untracked_mcp.is_empty() {
        eprintln!("✓ No untracked top-level MCP servers.");
    } else {
        eprintln!(
            "⚠ {} top-level MCP server(s) not referenced by any persona: {}",
            untracked_mcp.len(),
            untracked_mcp.join(", ")
        );
        eprintln!("    Fix: add them to a persona's [mcp] section, or `cc-persona snap`.");
    }

    // --- MCP source 2: enabled plugins may ship their own MCP servers ---
    let mcp_plugins = diagnostics::enabled_plugins(paths)?;
    if !mcp_plugins.is_empty() {
        eprintln!(
            "  {} enabled plugin(s) may provide MCP servers: {}",
            mcp_plugins.len(),
            mcp_plugins.join(", ")
        );
    }

    // --- MCP: persona patterns matching nothing (the v0.2 silent-no-op blind spot) ---
    // Source 3 (per-project connectors / claudeAiMcpEverConnected) feeds the union
    // these patterns are checked against; per-project disabled state is shown in the
    // Projects section below.
    let unmatched_patterns = diagnostics::unmatched_mcp_patterns(paths)?;
    if unmatched_patterns.is_empty() {
        eprintln!("✓ Every persona MCP pattern matches a known server/connector.");
    } else {
        eprintln!(
            "⚠ {} persona MCP pattern(s) match no known server/connector: {}",
            unmatched_patterns.len(),
            unmatched_patterns.join(", ")
        );
        eprintln!(
            "    Fix: these [mcp] patterns silently no-op. Reconcile them with names in"
        );
        eprintln!("         ~/.claude.json (mcpServers / claudeAiMcpEverConnected).");
    }

    // --- Snapshot presence (dirty-guard sanity) ---
    if active_name.is_some() {
        if diagnostics::snapshot_missing(paths) {
            eprintln!("⚠ Active-persona snapshot is missing (dirty-guard is dumb).");
            eprintln!(
                "    Fix: cc-persona use {}",
                active_name.as_deref().unwrap_or("<persona>")
            );
        } else {
            eprintln!("✓ Active-persona snapshot present.");
        }
    }

    // --- Project-scoped bindings (multi-window coexistence) ---
    let projects = diagnostics::projects_report(paths)?;
    if projects.bindings.is_empty() && projects.orphan_state_dirs.is_empty() {
        eprintln!("✓ No project-scoped bindings.");
    } else {
        eprintln!("  Project-scoped bindings:");
        for b in &projects.bindings {
            if b.dir_exists {
                // list_connectors reconciles b.path to Claude Code's literal key itself.
                let off: Vec<String> = mcp::list_connectors(&paths.claude_json, &b.path)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(_, disabled)| *disabled)
                    .map(|(name, _)| name)
                    .collect();
                if off.is_empty() {
                    eprintln!("    {} → {}", b.path, b.persona);
                } else {
                    eprintln!(
                        "    {} → {} (connectors off: {})",
                        b.path,
                        b.persona,
                        off.join(", ")
                    );
                }
            } else {
                eprintln!("⚠   {} → {} (project directory is gone)", b.path, b.persona);
                eprintln!("    Fix: cc-persona prune");
            }
        }
        for orphan in &projects.orphan_state_dirs {
            eprintln!("⚠   Orphan project state with no binding: {}", orphan);
            eprintln!("    Fix: cc-persona prune");
        }
    }

    // --- Backups usage (informational) ---
    let (backup_count, backup_bytes) = diagnostics::backups_usage(paths)?;
    eprintln!(
        "  {} backup(s), {}",
        backup_count,
        human_bytes(backup_bytes)
    );

    eprintln!();
    if errors > 0 {
        bail!(
            "{} problem(s) found. See the ✗ lines above for fixes.",
            errors
        );
    }
    eprintln!("✓ No problems found.");
    Ok(())
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;

    #[cfg(unix)]
    #[test]
    fn run_errors_on_legacy_symlink_dir() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        env.write_file(
            &crate::persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n",
        );
        let target = env.paths.skill_sets.join("engineer");
        std::fs::create_dir_all(&target).unwrap();
        env.symlink(&target, &env.paths.claude_skills);

        let err = run(&env.paths).unwrap_err();
        assert!(format!("{err:#}").contains("problem"));
    }

    #[cfg(unix)]
    #[test]
    fn run_is_clean_for_reconciled_state() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        std::fs::create_dir_all(&env.paths.claude_skills).unwrap();
        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        env.write_file(
            &crate::persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\n\n[skills]\nactive = [\"alpha\"]\n",
        );
        env.write_file(&env.paths.claude_settings, "{}");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.create_store_skill("cc-persona", "---\nname: cc-persona\n---\n");
        env.link_into_claude_skills("alpha");
        env.link_into_claude_skills("cc-persona");
        crate::active_persona::write_snapshot(&env.global_target(), "engineer").unwrap();

        run(&env.paths).unwrap();
    }
}
