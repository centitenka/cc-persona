use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Persona {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Inherit from another persona
    #[serde(default)]
    pub base: Option<String>,
    /// Overrides for ~/.claude/settings.json fields
    #[serde(default)]
    pub settings: Option<serde_json::Value>,
    /// Skills configuration
    #[serde(default)]
    pub skills: Option<SkillsConfig>,
    /// MCP server configuration
    #[serde(default)]
    pub mcp: Option<McpConfig>,
    /// CLAUDE.md configuration
    #[serde(default)]
    pub claude_md: Option<ClaudeMdConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SkillsConfig {
    /// Skills that should be active (others in skill-set dir will be disabled)
    #[serde(default)]
    pub active: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct McpConfig {
    /// MCP servers to enable
    #[serde(default)]
    pub enable: Vec<String>,
    /// MCP servers to disable
    #[serde(default)]
    pub disable: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ClaudeMdConfig {
    /// Filename in ~/.cc-persona/claude-md/
    #[serde(default)]
    pub file: Option<String>,
}

impl Persona {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read persona file: {}", path.display()))?;
        let persona: Persona = toml::from_str(&content)
            .with_context(|| format!("Failed to parse: {}", path.display()))?;
        Ok(persona)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content =
            toml::to_string_pretty(self).context("Failed to serialize persona to TOML")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Resolve the full persona by walking the inheritance chain (base → overrides).
    pub fn resolve(name: &str, personas_dir: &Path) -> Result<Persona> {
        let mut chain = Vec::new();
        let mut current_name = name.to_string();
        let mut visited = std::collections::HashSet::new();

        // Walk the inheritance chain
        loop {
            if !visited.insert(current_name.clone()) {
                bail!(
                    "Circular inheritance detected: {} already in chain",
                    current_name
                );
            }
            let path = personas_dir.join(format!("{}.toml", current_name));
            let persona = Persona::load(&path)?;
            let base = persona.base.clone();
            chain.push(persona);
            match base {
                Some(b) => current_name = b,
                None => break,
            }
        }

        // Merge from base → derived (chain is [derived, ..., base], reverse it)
        chain.reverse();
        let mut merged = chain.remove(0);
        for overlay in chain {
            merged = Self::merge(merged, overlay);
        }

        Ok(merged)
    }

    /// Merge overlay on top of base. Overlay fields take precedence where defined.
    fn merge(base: Persona, overlay: Persona) -> Persona {
        Persona {
            name: overlay.name,
            description: if overlay.description.is_empty() {
                base.description
            } else {
                overlay.description
            },
            base: overlay.base,
            settings: match (base.settings, overlay.settings) {
                (Some(b), Some(o)) => Some(json_merge(b, o)),
                (b, o) => o.or(b),
            },
            skills: overlay.skills.or(base.skills),
            mcp: overlay.mcp.or(base.mcp),
            claude_md: overlay.claude_md.or(base.claude_md),
        }
    }
}

/// Deep merge two JSON values. Overlay values win for non-object types.
fn json_merge(base: serde_json::Value, overlay: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match (base, overlay) {
        (Value::Object(mut b), Value::Object(o)) => {
            for (k, v) in o {
                let merged = if let Some(base_v) = b.remove(&k) {
                    json_merge(base_v, v)
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

/// List all persona names from the personas directory.
pub fn list_personas(personas_dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    if !personas_dir.exists() {
        return Ok(names);
    }
    for entry in std::fs::read_dir(personas_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml")
            && let Some(stem) = path.file_stem()
        {
            names.push(stem.to_string_lossy().to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// Get the persona file path.
pub fn persona_path(personas_dir: &Path, name: &str) -> PathBuf {
    personas_dir.join(format!("{}.toml", name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;
    use serde_json::json;

    #[test]
    fn resolve_merges_inheritance_and_replaces_non_settings_sections() {
        let env = TestEnv::new();
        std::fs::create_dir_all(&env.paths.personas).unwrap();

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
            &env.paths.personas.join("engineer.toml"),
            r#"
name = "engineer"
description = "Engineer persona"
base = "base"

[settings]
outputStyle = "Concise"

[settings.ui]
theme = "dark"

[skills]
active = ["alpha"]

[mcp]
enable = ["Linear"]
disable = ["Playwright"]

[claude_md]
file = "engineer.md"
"#,
        );

        let resolved = Persona::resolve("engineer", &env.paths.personas).unwrap();

        assert_eq!(resolved.name, "engineer");
        assert_eq!(resolved.description, "Engineer persona");
        assert_eq!(
            resolved.settings,
            Some(json!({
                "model": "claude-sonnet",
                "outputStyle": "Concise",
                "ui": {
                    "theme": "dark",
                    "font": "mono"
                }
            }))
        );
        assert_eq!(resolved.skills.unwrap().active, vec!["alpha"]);
        let mcp = resolved.mcp.unwrap();
        assert_eq!(mcp.enable, vec!["Linear"]);
        assert_eq!(mcp.disable, vec!["Playwright"]);
        assert_eq!(
            resolved.claude_md.unwrap().file.as_deref(),
            Some("engineer.md")
        );
    }

    #[test]
    fn resolve_detects_circular_inheritance() {
        let env = TestEnv::new();
        std::fs::create_dir_all(&env.paths.personas).unwrap();

        env.write_file(
            &env.paths.personas.join("a.toml"),
            "name = \"a\"\nbase = \"b\"\n",
        );
        env.write_file(
            &env.paths.personas.join("b.toml"),
            "name = \"b\"\nbase = \"a\"\n",
        );

        let err = Persona::resolve("a", &env.paths.personas).unwrap_err();
        assert!(format!("{err:#}").contains("Circular inheritance detected"));
    }

    #[test]
    fn list_personas_only_returns_sorted_toml_files() {
        let env = TestEnv::new();
        std::fs::create_dir_all(&env.paths.personas).unwrap();

        env.write_file(&env.paths.personas.join("zeta.toml"), "name = \"zeta\"\n");
        env.write_file(&env.paths.personas.join("alpha.toml"), "name = \"alpha\"\n");
        env.write_file(&env.paths.personas.join("notes.txt"), "ignore me");
        std::fs::create_dir_all(env.paths.personas.join("nested")).unwrap();

        let names = list_personas(&env.paths.personas).unwrap();
        assert_eq!(names, vec!["alpha".to_string(), "zeta".to_string()]);
    }
}
