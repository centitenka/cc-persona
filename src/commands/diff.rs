use anyhow::{Result, bail};

use crate::claude::settings;
use crate::config::{AppConfig, Paths, Scope};
use crate::persona::Persona;

pub fn run(paths: &Paths, scope: &Scope, name: Option<String>) -> Result<()> {
    let persona_name = match name {
        Some(n) => n,
        None => {
            let config = AppConfig::load(&paths.config)?;
            match config.binding(scope) {
                Some(n) => n.to_string(),
                None => bail!("No active persona. Specify a name: cc-persona diff <name>"),
            }
        }
    };

    let target = paths.resolve_target(scope);
    let resolved = Persona::resolve(&persona_name, &paths.personas)?;
    let current = settings::read_settings(&target.settings_file)?;

    println!(
        "=== Diff: current config vs persona '{}' ===\n",
        persona_name
    );

    // Settings diff
    if let Some(ref persona_settings) = resolved.settings {
        println!("[settings]");
        diff_json("", &current, persona_settings);
    }

    // Skills diff
    if let Some(ref skills_config) = resolved.skills {
        println!("\n[skills.active]");
        for s in &skills_config.active {
            println!("  {}", s);
        }
    }

    // MCP diff
    if let Some(ref mcp_config) = resolved.mcp {
        if !mcp_config.enable.is_empty() {
            println!("\n[mcp.enable]");
            for s in &mcp_config.enable {
                println!("  {}", s);
            }
        }
        if !mcp_config.disable.is_empty() {
            println!("[mcp.disable]");
            for s in &mcp_config.disable {
                println!("  {}", s);
            }
        }
    }

    // CLAUDE.md
    if let Some(ref md) = resolved.claude_md
        && let Some(ref file) = md.file
    {
        println!("\n[claude_md]");
        println!("  file = {}", file);
    }

    Ok(())
}

fn diff_json(prefix: &str, current: &serde_json::Value, target: &serde_json::Value) {
    use serde_json::Value;
    match (current, target) {
        (Value::Object(c), Value::Object(t)) => {
            for (key, target_val) in t {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                match c.get(key) {
                    Some(current_val) if current_val == target_val => {
                        println!("  {} = {} (unchanged)", path, format_value(target_val));
                    }
                    Some(current_val) => {
                        if current_val.is_object() && target_val.is_object() {
                            diff_json(&path, current_val, target_val);
                        } else {
                            println!(
                                "  {} : {} → {}",
                                path,
                                format_value(current_val),
                                format_value(target_val)
                            );
                        }
                    }
                    None => {
                        println!("  {} : (unset) → {}", path, format_value(target_val));
                    }
                }
            }
        }
        _ => {
            if current != target {
                println!(
                    "  {} : {} → {}",
                    prefix,
                    format_value(current),
                    format_value(target)
                );
            }
        }
    }
}

fn format_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("\"{}\"", s),
        other => other.to_string(),
    }
}
