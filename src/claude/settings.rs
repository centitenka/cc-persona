use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Read settings.json as a JSON Value.
pub fn read_settings(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let content = std::fs::read_to_string(path).context("Failed to read settings.json")?;
    let value: Value = serde_json::from_str(&content).context("Failed to parse settings.json")?;
    Ok(value)
}

/// Write a JSON Value to settings.json (pretty-printed).
pub fn write_settings(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(value).context("Failed to serialize settings")?;
    std::fs::write(path, content)?;
    Ok(())
}

/// Apply persona settings overrides onto current settings.
/// The overrides are merged deeply: object fields are merged recursively,
/// non-object fields are replaced.
pub fn apply_overrides(current: &Value, overrides: &Value) -> Value {
    deep_merge(current.clone(), overrides.clone())
}

fn deep_merge(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Object(mut b), Value::Object(o)) => {
            for (k, v) in o {
                let merged = if let Some(base_v) = b.remove(&k) {
                    deep_merge(base_v, v)
                } else {
                    v
                };
                b.insert(k, merged);
            }
            Value::Object(b)
        }
        (_, overlay) => overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_overrides_deep_merges_objects_and_replaces_non_objects() {
        let current = json!({
            "model": "claude-sonnet",
            "ui": {
                "theme": "light",
                "font": "mono"
            },
            "tools": ["bash"],
            "streaming": false
        });
        let overrides = json!({
            "ui": {
                "theme": "dark"
            },
            "tools": ["bash", "edit"],
            "streaming": true,
            "newFlag": {
                "enabled": true
            }
        });

        let merged = apply_overrides(&current, &overrides);

        assert_eq!(
            merged,
            json!({
                "model": "claude-sonnet",
                "ui": {
                    "theme": "dark",
                    "font": "mono"
                },
                "tools": ["bash", "edit"],
                "streaming": true,
                "newFlag": {
                    "enabled": true
                }
            })
        );
    }
}
