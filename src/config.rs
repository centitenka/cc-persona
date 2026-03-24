use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// cc-persona's own configuration stored at ~/.cc-persona/config.toml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    /// Currently active persona name
    #[serde(default)]
    pub active_persona: Option<String>,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path).context("Failed to read config.toml")?;
        let config: AppConfig = toml::from_str(&content).context("Failed to parse config.toml")?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

/// All cc-persona paths, resolved from home directory.
#[derive(Debug, Clone)]
pub struct Paths {
    /// ~/.cc-persona/
    pub root: PathBuf,
    /// ~/.cc-persona/config.toml
    pub config: PathBuf,
    /// ~/.cc-persona/personas/
    pub personas: PathBuf,
    /// ~/.cc-persona/skill-sets/
    pub skill_sets: PathBuf,
    /// ~/.cc-persona/claude-md/
    pub claude_md: PathBuf,
    /// ~/.cc-persona/backups/
    pub backups: PathBuf,
    /// ~/.claude/settings.json
    pub claude_settings: PathBuf,
    /// ~/.claude/skills/
    pub claude_skills: PathBuf,
    /// ~/.claude/CLAUDE.md
    pub claude_md_file: PathBuf,
    /// ~/.claude.json
    pub claude_json: PathBuf,
}

impl Paths {
    pub fn new() -> Result<Self> {
        let home = dirs::home_dir().context("Cannot determine home directory")?;
        let root = home.join(".cc-persona");
        let claude_dir = home.join(".claude");
        Ok(Self {
            config: root.join("config.toml"),
            personas: root.join("personas"),
            skill_sets: root.join("skill-sets"),
            claude_md: root.join("claude-md"),
            backups: root.join("backups"),
            root,
            claude_settings: claude_dir.join("settings.json"),
            claude_skills: claude_dir.join("skills"),
            claude_md_file: claude_dir.join("CLAUDE.md"),
            claude_json: home.join(".claude.json"),
        })
    }

    /// Ensure all cc-persona directories exist.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            &self.root,
            &self.personas,
            &self.skill_sets,
            &self.claude_md,
            &self.backups,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}
