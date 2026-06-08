use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

use crate::claude::{mcp, settings, skills};
use crate::config::{AppConfig, Paths, Scope, Target};
use crate::persona::{self, ClaudeMdConfig, McpConfig, Persona, SkillsConfig};

type SkillStatuses = Vec<(String, bool)>;
type SkillState = (Vec<String>, SkillStatuses);
type McpState = (Vec<String>, Vec<String>, Vec<String>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistChoice {
    Save,
    Discard,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct ActivePersonaState {
    persona_name: String,
    settings: Value,
    skills_active: Vec<String>,
    mcp_enable: Vec<String>,
    mcp_disable: Vec<String>,
    claude_md_content: String,
}

pub fn persist_choice(save_current: bool, discard_current: bool) -> Option<PersistChoice> {
    if save_current {
        Some(PersistChoice::Save)
    } else if discard_current {
        Some(PersistChoice::Discard)
    } else {
        None
    }
}

pub fn guard_and_handle_dirty(
    paths: &Paths,
    target: &Target,
    scope: &Scope,
    choice: Option<PersistChoice>,
    rerun_command: &str,
) -> Result<()> {
    let config = AppConfig::load(&paths.config)?;
    let Some(active_persona) = config.binding(scope).map(str::to_string) else {
        return Ok(());
    };

    if !is_dirty(target, &active_persona)? {
        return Ok(());
    }

    match choice {
        Some(PersistChoice::Save) => {
            save_current_persona(paths, target, &active_persona)?;
            write_snapshot(target, &active_persona)?;
            eprintln!("✓ Saved current persona '{}'", active_persona);
            Ok(())
        }
        Some(PersistChoice::Discard) => Ok(()),
        None => bail!(
            "Current persona '{}' has unsaved changes. Re-run `{}` with `--save-current` \
             (recommended) or `--discard-current`.",
            active_persona,
            rerun_command
        ),
    }
}

pub fn write_snapshot(target: &Target, persona_name: &str) -> Result<()> {
    let state = capture_live_state(target, persona_name)?;
    if let Some(parent) = target.snapshot_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content =
        serde_json::to_string_pretty(&state).context("Failed to serialize active persona state")?;
    std::fs::write(&target.snapshot_path, content)
        .context("Failed to write active persona state")?;
    Ok(())
}

pub fn clear_snapshot(target: &Target) -> Result<()> {
    if target.snapshot_path.exists() {
        std::fs::remove_file(&target.snapshot_path)
            .context("Failed to remove active persona state")?;
    }
    Ok(())
}

fn is_dirty(target: &Target, persona_name: &str) -> Result<bool> {
    let Some(snapshot) = load_snapshot(target)? else {
        return Ok(false);
    };
    if snapshot.persona_name != persona_name {
        return Ok(false);
    }

    Ok(snapshot != capture_live_state(target, persona_name)?)
}

fn load_snapshot(target: &Target) -> Result<Option<ActivePersonaState>> {
    if !target.snapshot_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&target.snapshot_path)
        .context("Failed to read active persona state")?;
    let state = serde_json::from_str(&content).context("Failed to parse active persona state")?;
    Ok(Some(state))
}

fn capture_live_state(target: &Target, persona_name: &str) -> Result<ActivePersonaState> {
    let (skills_active, _) = current_skills(&target.skills_dir)?;
    let (mcp_enable, mcp_disable, _) = current_mcp(target)?;
    let claude_md_content = read_current_claude_md(target)?;

    Ok(ActivePersonaState {
        persona_name: persona_name.to_string(),
        settings: settings::read_settings(&target.settings_file)?,
        skills_active,
        mcp_enable,
        mcp_disable,
        claude_md_content,
    })
}

fn save_current_persona(paths: &Paths, target: &Target, persona_name: &str) -> Result<()> {
    let persona_file = persona::persona_path(&paths.personas, persona_name);
    let mut persona = Persona::load(&persona_file)
        .with_context(|| format!("Failed to load current persona '{}'", persona_name))?;

    let current_settings = settings::read_settings(&target.settings_file)?;
    let (skills_active, _) = current_skills(&target.skills_dir)?;
    let (mcp_enable, mcp_disable, mcp_names) = current_mcp(target)?;
    let current_md_content = read_current_claude_md(target)?;

    let base_resolved = match persona.base.as_deref() {
        Some(base_name) => Some(Persona::resolve(base_name, &paths.personas)?),
        None => None,
    };

    let base_settings = base_resolved
        .as_ref()
        .and_then(|base| base.settings.clone())
        .unwrap_or_else(empty_object);
    persona.settings = json_overlay(&base_settings, &current_settings);

    let base_skills = normalize_names(
        base_resolved
            .as_ref()
            .and_then(|base| base.skills.as_ref())
            .map(|skills| skills.active.as_slice())
            .unwrap_or(&[]),
    );
    let current_skills = normalize_names(&skills_active);
    persona.skills = if current_skills == base_skills {
        None
    } else {
        Some(SkillsConfig {
            active: current_skills,
        })
    };

    let base_mcp = materialize_mcp_state(
        base_resolved.as_ref().and_then(|base| base.mcp.as_ref()),
        &mcp_names,
    );
    let current_mcp = McpConfig {
        enable: mcp_enable,
        disable: mcp_disable,
    };
    persona.mcp = if normalize_mcp(&current_mcp) == normalize_mcp(&base_mcp) {
        None
    } else {
        Some(normalize_mcp(&current_mcp))
    };

    // CLAUDE.md is only captured at scopes that manage it (global/window). At
    // project scope it is a user-level concern, so the persona's claude_md is
    // left untouched.
    if target.claude_md_file.is_some() {
        let md_filename = persona
            .claude_md
            .as_ref()
            .and_then(|md| md.file.clone())
            .unwrap_or_else(|| format!("{}.md", persona_name));
        let md_target = paths.claude_md.join(&md_filename);
        if let Some(parent) = md_target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&md_target, current_md_content)
            .with_context(|| format!("Failed to save {}", md_target.display()))?;
        persona.claude_md = Some(ClaudeMdConfig {
            file: Some(md_filename),
        });
    }

    persona.save(&persona_file)?;
    Ok(())
}

fn current_skills(skills_dir: &Path) -> Result<SkillState> {
    // The skills dir is a real directory; managed skills are per-skill symlinks
    // into the store. Active = managed links that are not muted. Wild (real)
    // subdirectories are untracked and never count as active — this fixes the
    // false-dirty bug where externally-installed skills inflated the active set.
    let entries = skills::list_skills_ext(skills_dir)?;
    let statuses: SkillStatuses = entries
        .iter()
        .filter(|e| e.managed)
        .map(|e| (e.name.clone(), e.disabled))
        .collect();
    let active = statuses
        .iter()
        .filter(|(_, disabled)| !disabled)
        .map(|(name, _)| name.clone())
        .collect();
    Ok((active, statuses))
}

/// Capture the live MCP state for dirty detection. Global scope reads the shared
/// top-level `mcpServers`; project scope reads its own connector list (so a global
/// MCP change never makes every project's snapshot dirty).
fn current_mcp(target: &Target) -> Result<McpState> {
    match &target.claude_json_project_key {
        None => {
            let listed = mcp::list_mcp_servers(&target.claude_json)?;
            let enable = listed
                .iter()
                .filter(|(_, disabled)| !disabled)
                .map(|(name, _)| name.clone())
                .collect();
            let disable = listed
                .iter()
                .filter(|(_, disabled)| *disabled)
                .map(|(name, _)| name.clone())
                .collect();
            let names = listed.into_iter().map(|(name, _)| name).collect();
            Ok((enable, disable, names))
        }
        Some(key) => {
            let mut disabled = mcp::list_disabled_connectors(&target.claude_json, key)?;
            disabled.sort();
            let names = disabled.clone();
            Ok((Vec::new(), disabled, names))
        }
    }
}

fn read_current_claude_md(target: &Target) -> Result<String> {
    let Some(md_file) = &target.claude_md_file else {
        return Ok(String::new());
    };
    let is_symlink = std::fs::symlink_metadata(md_file)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if !md_file.exists() && !is_symlink {
        return Ok(String::new());
    }
    std::fs::read_to_string(md_file).context("Failed to read CLAUDE.md")
}

fn materialize_mcp_state(config: Option<&McpConfig>, server_names: &[String]) -> McpConfig {
    let Some(config) = config else {
        return McpConfig::default();
    };

    let mut enable = Vec::new();
    let mut disable = Vec::new();
    for name in server_names {
        if config.enable.iter().any(|pattern| name.contains(pattern)) {
            enable.push(name.clone());
        } else if config.disable.iter().any(|pattern| name.contains(pattern)) {
            disable.push(name.clone());
        }
    }

    McpConfig { enable, disable }
}

fn normalize_names(names: &[String]) -> Vec<String> {
    let mut normalized = names.to_vec();
    normalized.sort();
    normalized
}

fn normalize_mcp(config: &McpConfig) -> McpConfig {
    let mut normalized = config.clone();
    normalized.enable.sort();
    normalized.disable.sort();
    normalized
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

fn json_overlay(base: &Value, current: &Value) -> Option<Value> {
    match (base, current) {
        (Value::Object(base_map), Value::Object(current_map)) => {
            let mut overlay = Map::new();

            for (key, current_value) in current_map {
                match base_map.get(key) {
                    Some(base_value) => {
                        if let Some(diff) = json_overlay(base_value, current_value) {
                            overlay.insert(key.clone(), diff);
                        }
                    }
                    None => {
                        overlay.insert(key.clone(), current_value.clone());
                    }
                }
            }

            if overlay.is_empty() {
                None
            } else {
                Some(Value::Object(overlay))
            }
        }
        _ if base == current => None,
        _ => Some(current.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::skills::switch_skills_symlink;
    use crate::test_support::TestEnv;
    use serde_json::json;

    #[test]
    fn json_overlay_only_keeps_changed_keys() {
        let base = json!({
            "model": "claude-sonnet",
            "ui": {
                "theme": "light",
                "font": "mono"
            }
        });
        let current = json!({
            "ui": {
                "theme": "dark"
            }
        });

        assert_eq!(
            json_overlay(&base, &current),
            Some(json!({
                "ui": {
                    "theme": "dark"
                }
            }))
        );
    }

    #[cfg(unix)]
    #[test]
    fn guard_detects_dirty_persona_from_snapshot() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();
        env.write_file(
            &persona::persona_path(&env.paths.personas, "engineer"),
            "name = \"engineer\"\ndescription = \"Engineer\"\n",
        );
        env.write_file(&env.paths.config, "active_persona = \"engineer\"\n");
        env.write_file(&env.paths.claude_settings, "{}");
        env.write_file(&env.paths.claude_json, "{\"mcpServers\":{}}");
        env.write_file(&env.paths.claude_md_file, "base");
        std::fs::create_dir_all(env.paths.skill_sets.join("engineer")).unwrap();
        switch_skills_symlink(&env.paths, "engineer").unwrap();

        write_snapshot(&env.global_target(), "engineer").unwrap();
        env.write_file(&env.paths.claude_md_file, "changed");

        let err = guard_and_handle_dirty(
            &env.paths,
            &env.global_target(),
            &Scope::Global,
            None,
            "cc-persona off",
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("--save-current"));
    }

    #[cfg(unix)]
    #[test]
    fn save_current_persona_preserves_base_and_updates_overlay() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();

        env.write_file(
            &env.paths.personas.join("base.toml"),
            r#"
name = "base"
description = "Base persona"

[settings]
model = "claude-sonnet"

[settings.ui]
theme = "light"
font = "mono"

[skills]
active = ["base-skill"]

[mcp]
enable = ["GitHub"]
disable = ["Figma"]

[claude_md]
file = "base.md"
"#,
        );
        env.write_file(
            &env.paths.personas.join("derived.toml"),
            r#"
name = "derived"
description = "Derived persona"
base = "base"
"#,
        );
        env.write_file(
            &env.paths.claude_settings,
            &json!({
                "model": "claude-sonnet",
                "ui": {
                    "theme": "dark",
                    "font": "mono"
                },
                "newFlag": true
            })
            .to_string(),
        );
        env.write_file(
            &env.paths.claude_json,
            &json!({
                "mcpServers": {
                    "GitHub Prod": { "command": "github" },
                    "Figma Design": { "command": "figma", "disabled": true },
                    "Linear": { "command": "linear" }
                }
            })
            .to_string(),
        );
        env.write_file(&env.paths.claude_md_file, "derived instructions");

        // New model: ~/.claude/skills is a real directory holding per-skill
        // symlinks into the store. `alpha` is active; `beta` is managed but muted
        // (disable-model-invocation in its store SKILL.md), so it must not appear
        // in the captured active set.
        env.create_store_skill("alpha", "---\nname: alpha\n---\n");
        env.create_store_skill(
            "beta",
            "---\nname: beta\ndisable-model-invocation: true\n---\n",
        );
        env.link_into_claude_skills("alpha");
        env.link_into_claude_skills("beta");

        save_current_persona(&env.paths, &env.global_target(), "derived").unwrap();

        let saved = Persona::load(&env.paths.personas.join("derived.toml")).unwrap();
        assert_eq!(saved.base.as_deref(), Some("base"));
        assert_eq!(
            saved.settings,
            Some(json!({
                "newFlag": true,
                "ui": {
                    "theme": "dark"
                }
            }))
        );
        assert_eq!(saved.skills.unwrap().active, vec!["alpha"]);
        let mcp = saved.mcp.unwrap();
        assert_eq!(mcp.enable, vec!["GitHub Prod", "Linear"]);
        assert_eq!(mcp.disable, vec!["Figma Design"]);
        assert_eq!(saved.claude_md.unwrap().file.as_deref(), Some("derived.md"));
        assert_eq!(
            env.read_file(&env.paths.claude_md.join("derived.md")),
            "derived instructions"
        );
    }
}
