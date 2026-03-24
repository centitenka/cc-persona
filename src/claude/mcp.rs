use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

use crate::persona::McpConfig;

/// Read ~/.claude.json and return the full JSON value.
pub fn read_claude_json(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let content = std::fs::read_to_string(path).context("Failed to read ~/.claude.json")?;
    let value: Value = serde_json::from_str(&content).context("Failed to parse ~/.claude.json")?;
    Ok(value)
}

/// Write the full JSON value back to ~/.claude.json (pretty-printed).
pub fn write_claude_json(path: &Path, value: &Value) -> Result<()> {
    let content =
        serde_json::to_string_pretty(value).context("Failed to serialize ~/.claude.json")?;
    std::fs::write(path, content)?;
    Ok(())
}

/// Apply MCP enable/disable configuration.
/// Looks for mcpServers in the JSON and sets "disabled" field.
pub fn apply_mcp_config(claude_json: &mut Value, mcp: &McpConfig) -> Result<()> {
    let servers = match claude_json.get_mut("mcpServers") {
        Some(Value::Object(map)) => map,
        _ => return Ok(()), // No mcpServers section, nothing to do
    };

    for (name, server) in servers.iter_mut() {
        let server_obj = match server.as_object_mut() {
            Some(o) => o,
            None => continue,
        };

        if mcp.enable.iter().any(|e| name.contains(e.as_str())) {
            server_obj.remove("disabled");
        } else if mcp.disable.iter().any(|d| name.contains(d.as_str())) {
            server_obj.insert("disabled".to_string(), Value::Bool(true));
        }
    }

    Ok(())
}

/// List all MCP server names and their disabled status.
pub fn list_mcp_servers(path: &Path) -> Result<Vec<(String, bool)>> {
    let json = read_claude_json(path)?;
    let mut result = Vec::new();
    if let Some(Value::Object(servers)) = json.get("mcpServers") {
        for (name, server) in servers {
            let disabled = server
                .get("disabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            result.push((name.clone(), disabled));
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_mcp_config_toggles_matching_servers_only() {
        let mut claude_json = json!({
            "mcpServers": {
                "GitHub Prod": {
                    "command": "github",
                    "disabled": true
                },
                "Figma Design": {
                    "command": "figma"
                },
                "Linear": {
                    "command": "linear",
                    "disabled": true
                }
            }
        });
        let config = McpConfig {
            enable: vec!["GitHub".to_string()],
            disable: vec!["Figma".to_string()],
        };

        apply_mcp_config(&mut claude_json, &config).unwrap();

        let servers = claude_json["mcpServers"].as_object().unwrap();
        assert!(servers["GitHub Prod"].get("disabled").is_none());
        assert_eq!(servers["Figma Design"]["disabled"], Value::Bool(true));
        assert_eq!(servers["Linear"]["disabled"], Value::Bool(true));
    }

    #[test]
    fn apply_mcp_config_is_noop_without_mcp_servers() {
        let original = json!({
            "version": 1
        });
        let mut claude_json = original.clone();
        let config = McpConfig {
            enable: vec!["GitHub".to_string()],
            disable: vec!["Figma".to_string()],
        };

        apply_mcp_config(&mut claude_json, &config).unwrap();

        assert_eq!(claude_json, original);
    }
}
