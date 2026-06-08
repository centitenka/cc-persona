use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cc-persona",
    version,
    about = "Fast persona switching for Claude Code"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize cc-persona: create directories and install Claude Code skill
    Init,

    /// List all personas (mark active one)
    #[command(alias = "ls")]
    List,

    /// Switch to a persona (interactive select if no name given)
    Use {
        /// Persona name
        name: Option<String>,
        /// Apply to the current project (`<cwd>/.claude/`) instead of globally
        #[arg(long)]
        project: bool,
        /// Save current active persona changes before switching
        #[arg(long, conflicts_with = "discard_current")]
        save_current: bool,
        /// Discard current active persona changes before switching
        #[arg(long, conflicts_with = "save_current")]
        discard_current: bool,
    },

    /// Restore original configuration (undo last switch)
    Off {
        /// Restore the current project scope instead of the global scope
        #[arg(long)]
        project: bool,
        /// Save current active persona changes before restoring
        #[arg(long, conflicts_with = "discard_current")]
        save_current: bool,
        /// Discard current active persona changes before restoring
        #[arg(long, conflicts_with = "save_current")]
        discard_current: bool,
    },

    /// Launch a Claude Code window with an isolated, persona-scoped config dir
    /// (EXPERIMENTAL — relies on the undocumented CLAUDE_CONFIG_DIR).
    Shell {
        /// Persona name to materialize into the isolated window
        name: String,
    },

    /// Remove stale project bindings whose directories no longer exist
    Prune,

    /// Create a new persona
    Create {
        /// Persona name
        name: String,
    },

    /// Snapshot current Claude Code config into a persona
    Snap {
        /// Persona name (default: auto-generated)
        name: Option<String>,
        /// Snapshot the current project scope instead of the global scope
        #[arg(long)]
        project: bool,
    },

    /// Open persona TOML in $EDITOR
    Edit {
        /// Persona name
        name: String,
    },

    /// Show the fully resolved config of a persona
    Show {
        /// Persona name (default: active persona)
        name: Option<String>,
        /// Resolve against the current project scope's active persona
        #[arg(long)]
        project: bool,
    },

    /// Diff current config against a persona
    Diff {
        /// Persona name (default: active persona)
        name: Option<String>,
        /// Diff against the current project scope's config
        #[arg(long)]
        project: bool,
    },

    /// Show currently active persona
    Which {
        /// Report the current project scope's binding instead of the global one
        #[arg(long)]
        project: bool,
    },

    /// Manage skills within the current persona
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },

    /// Diagnose drift, untracked skills/plugins/MCP, and snapshot health
    #[command(alias = "status")]
    Doctor,

    /// Adopt untracked skills into a persona (move into the store + add to active)
    Adopt {
        /// Target persona (default: active persona)
        #[arg(long)]
        into: Option<String>,
        /// Skill names to adopt (interactive multi-select if omitted)
        #[arg(num_args = 0..)]
        names: Vec<String>,
    },

    /// Migrate a v0.1 layout to the v0.2 shared-store + per-skill-link model
    Migrate {
        /// Copy skill-sets into the store instead of moving them
        #[arg(long)]
        copy: bool,
    },
}

#[derive(Subcommand)]
pub enum SkillAction {
    /// List skills and their status
    #[command(alias = "ls")]
    List,

    /// Toggle a skill's disable-model-invocation
    Toggle {
        /// Skill name
        name: String,
    },

    /// Remove a skill permanently
    Rm {
        /// Skill name
        name: String,
    },
}
