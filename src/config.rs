use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The binding of a persona to a single project directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectBinding {
    /// Persona name active in this project scope.
    pub persona: String,
}

/// cc-persona's own configuration stored at ~/.cc-persona/config.toml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    /// Persona active at GLOBAL (user-level) scope.
    #[serde(default)]
    pub active_persona: Option<String>,
    /// Per-project bindings, keyed by absolute (canonicalized) project directory.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub projects: BTreeMap<String, ProjectBinding>,
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

    /// The persona bound to the given scope, if any.
    pub fn binding(&self, scope: &Scope) -> Option<&str> {
        match scope {
            Scope::Global => self.active_persona.as_deref(),
            Scope::Project(cwd) => self
                .projects
                .get(&project_key(cwd))
                .map(|b| b.persona.as_str()),
        }
    }

    /// Bind (or, with `None`, clear) the persona for the given scope.
    pub fn set_binding(&mut self, scope: &Scope, persona: Option<String>) {
        match scope {
            Scope::Global => self.active_persona = persona,
            Scope::Project(cwd) => {
                let key = project_key(cwd);
                match persona {
                    Some(name) => {
                        self.projects.insert(key, ProjectBinding { persona: name });
                    }
                    None => {
                        self.projects.remove(&key);
                    }
                }
            }
        }
    }
}

/// Where a persona switch applies. Global is the user-level config (today's
/// behaviour, also used inside a `CLAUDE_CONFIG_DIR`-isolated window); Project
/// targets `<cwd>/.claude/` so multiple windows on different projects coexist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Global,
    /// Absolute, canonicalized project directory.
    Project(PathBuf),
}

impl Scope {
    pub fn is_global(&self) -> bool {
        matches!(self, Scope::Global)
    }
}

/// The string key used for a project directory in `AppConfig.projects` and in
/// `~/.claude.json`'s `projects` map — the directory's path as a string.
pub fn project_key(cwd: &Path) -> String {
    cwd.to_string_lossy().into_owned()
}

/// A resolved set of Claude Code config destinations + scope-specific cc-persona
/// state, computed from an [`AppPaths`]/[`Paths`] and a [`Scope`]. Apply/backup/
/// dirty logic operates on a `Target`, never on raw `Paths.claude_*` fields, so
/// the same code drives every scope.
#[derive(Debug, Clone)]
pub struct Target {
    /// settings.json (global) or settings.local.json (project).
    pub settings_file: PathBuf,
    /// The `skills/` directory of per-skill symlinks into the shared store.
    pub skills_dir: PathBuf,
    /// CLAUDE.md destination, or `None` when this scope does not manage CLAUDE.md
    /// (project scope: CLAUDE.md is a user-level concern only).
    pub claude_md_file: Option<PathBuf>,
    /// The `~/.claude.json` file (shared across scopes).
    pub claude_json: PathBuf,
    /// The `projects.<key>` entry for per-project connectors, or `None` (global).
    pub claude_json_project_key: Option<String>,
    /// Per-scope dirty snapshot path.
    pub snapshot_path: PathBuf,
    /// Per-scope backups directory.
    pub backups_dir: PathBuf,
    /// Whether the built-in `cc-persona` skill is force-linked into `skills_dir`.
    /// True at global/window scope; false at project scope (the user-level link
    /// already provides it, and project + user skills merge in Claude Code).
    pub include_cc_persona_skill: bool,
}

/// Metadata describing a project scope, stored at
/// `~/.cc-persona/projects/<enc>/meta.json` so `doctor`/`prune` can enumerate
/// bindings and detect stale ones without a shared central index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub project_path: String,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub last_used: Option<String>,
}

/// All cc-persona paths, resolved from home directory.
#[derive(Debug, Clone)]
pub struct Paths {
    /// ~/.cc-persona/
    pub root: PathBuf,
    /// ~/.cc-persona/config.toml
    pub config: PathBuf,
    /// ~/.cc-persona/active-persona-state.json
    pub active_persona_state: PathBuf,
    /// ~/.cc-persona/personas/
    pub personas: PathBuf,
    /// ~/.cc-persona/skill-sets/
    pub skill_sets: PathBuf,
    /// ~/.cc-persona/skill-store/
    pub skill_store: PathBuf,
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
        // The Claude config base honours $CLAUDE_CONFIG_DIR (default ~/.claude).
        // This is what makes the experimental window scope work: a `cc-persona
        // shell`-launched window sets CLAUDE_CONFIG_DIR to an isolated dir, and a
        // plain global apply then targets that dir automatically.
        let (claude_dir, claude_json) = claude_config_base(&home);
        Ok(Self {
            config: root.join("config.toml"),
            active_persona_state: root.join("active-persona-state.json"),
            personas: root.join("personas"),
            skill_sets: root.join("skill-sets"),
            skill_store: root.join("skill-store"),
            claude_md: root.join("claude-md"),
            backups: root.join("backups"),
            root,
            claude_settings: claude_dir.join("settings.json"),
            claude_skills: claude_dir.join("skills"),
            claude_md_file: claude_dir.join("CLAUDE.md"),
            claude_json,
        })
    }

    /// Ensure all cc-persona directories exist.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            &self.root,
            &self.personas,
            &self.skill_sets,
            &self.skill_store,
            &self.claude_md,
            &self.backups,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// The GLOBAL (user-level) target — today's behaviour, byte-for-byte.
    pub fn global_target(&self) -> Target {
        Target {
            settings_file: self.claude_settings.clone(),
            skills_dir: self.claude_skills.clone(),
            claude_md_file: Some(self.claude_md_file.clone()),
            claude_json: self.claude_json.clone(),
            claude_json_project_key: None,
            snapshot_path: self.active_persona_state.clone(),
            backups_dir: self.backups.clone(),
            include_cc_persona_skill: true,
        }
    }

    /// Resolve the [`Target`] for a scope. Global returns [`Self::global_target`];
    /// Project targets `<cwd>/.claude/` with cc-persona state kept out of the repo
    /// under `~/.cc-persona/projects/<enc>/`.
    pub fn resolve_target(&self, scope: &Scope) -> Target {
        match scope {
            Scope::Global => self.global_target(),
            Scope::Project(cwd) => {
                let claude_dir = cwd.join(".claude");
                let state_root = self.project_state_root(cwd);
                Target {
                    settings_file: claude_dir.join("settings.local.json"),
                    skills_dir: claude_dir.join("skills"),
                    claude_md_file: None,
                    claude_json: self.claude_json.clone(),
                    claude_json_project_key: Some(project_key(cwd)),
                    snapshot_path: state_root.join("active-persona-state.json"),
                    backups_dir: state_root.join("backups"),
                    include_cc_persona_skill: false,
                }
            }
        }
    }

    /// `~/.cc-persona/projects/<enc>/` — the per-project state root for `cwd`.
    pub fn project_state_root(&self, cwd: &Path) -> PathBuf {
        self.root.join("projects").join(encode_project_dir(cwd))
    }

    /// `~/.cc-persona/projects/` — holds one state dir per bound project.
    pub fn projects_root(&self) -> PathBuf {
        self.root.join("projects")
    }
}

/// Resolve the Claude config directory and `.claude.json` path, honouring
/// `$CLAUDE_CONFIG_DIR`. When unset, the defaults are `~/.claude` and
/// `~/.claude.json`. When set, both move under the custom dir (best-effort:
/// `CLAUDE_CONFIG_DIR` is undocumented and its `.claude.json` placement may vary
/// across Claude Code versions — this is used only for the experimental window scope).
fn claude_config_base(home: &Path) -> (PathBuf, PathBuf) {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => {
            let dir = PathBuf::from(dir);
            let json = dir.join(".claude.json");
            (dir, json)
        }
        _ => (home.join(".claude"), home.join(".claude.json")),
    }
}

/// Encode an absolute project dir into a filesystem-safe, length-bounded,
/// collision-free directory name: a sanitized path stem + an 8-hex hash of the
/// full path. Human-greppable in `~/.cc-persona/projects/`, unique per path.
pub fn encode_project_dir(cwd: &Path) -> String {
    let full = cwd.to_string_lossy();
    let sanitized: String = full
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let stem = sanitized.trim_matches('-');
    // Keep the tail (most specific) part of the path, bounded to 40 chars.
    let tail: String = {
        let chars: Vec<char> = stem.chars().collect();
        let start = chars.len().saturating_sub(40);
        chars[start..].iter().collect()
    };
    format!("{}-{:08x}", tail.trim_matches('-'), fnv1a_hash(full.as_bytes()))
}

/// FNV-1a 32-bit hash — small, dependency-free, deterministic. Used only to
/// disambiguate encoded project dir names, never for security.
fn fnv1a_hash(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;

    #[test]
    fn load_returns_default_when_file_missing() {
        let env = TestEnv::new();
        let missing = env.paths.root.join("does-not-exist").join("config.toml");
        assert!(!missing.exists());

        let config = AppConfig::load(&missing).unwrap();

        assert_eq!(config.active_persona, None);
    }

    #[test]
    fn save_then_load_round_trips_active_persona() {
        let env = TestEnv::new();
        let config = AppConfig {
            active_persona: Some("engineer".to_string()),
            ..Default::default()
        };

        config.save(&env.paths.config).unwrap();
        let loaded = AppConfig::load(&env.paths.config).unwrap();

        assert_eq!(loaded.active_persona, Some("engineer".to_string()));
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let env = TestEnv::new();
        let nested = env
            .paths
            .root
            .join("deeply")
            .join("nested")
            .join("config.toml");
        assert!(!nested.parent().unwrap().exists());

        AppConfig::default().save(&nested).unwrap();

        assert!(nested.exists());
    }

    #[test]
    fn ensure_dirs_creates_all_directories_including_skill_store() {
        let env = TestEnv::new();
        assert!(!env.paths.skill_store.exists());

        env.paths.ensure_dirs().unwrap();

        for dir in [
            &env.paths.root,
            &env.paths.personas,
            &env.paths.skill_sets,
            &env.paths.skill_store,
            &env.paths.claude_md,
            &env.paths.backups,
        ] {
            assert!(
                dir.is_dir(),
                "expected directory to exist: {}",
                dir.display()
            );
        }
    }

    #[test]
    fn ensure_dirs_is_idempotent_when_run_twice() {
        let env = TestEnv::new();

        env.paths.ensure_dirs().unwrap();
        env.paths.ensure_dirs().unwrap();

        assert!(env.paths.skill_store.is_dir());
    }
}
