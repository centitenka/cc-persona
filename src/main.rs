mod active_persona;
mod backup;
mod claude;
mod cli;
mod commands;
mod config;
mod diagnostics;
mod persona;
mod symlink;
#[cfg(test)]
mod test_support;

use anyhow::{Context, Result};
use clap::Parser;

use cli::{Cli, Commands, SkillAction};
use config::{Paths, Scope};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::new()?;

    match cli.command {
        Some(Commands::Init) => commands::init::run(&paths),
        Some(Commands::List) => commands::list::run(&paths),
        Some(Commands::Use {
            name,
            project,
            save_current,
            discard_current,
        }) => commands::use_cmd::run(&paths, &scope(project)?, name, save_current, discard_current),
        Some(Commands::Off {
            project,
            save_current,
            discard_current,
        }) => commands::off::run(&paths, &scope(project)?, save_current, discard_current),
        Some(Commands::Shell { name }) => commands::shell::run(&paths, &name),
        Some(Commands::Prune) => commands::prune::run(&paths),
        Some(Commands::Create { name }) => commands::create::run(&paths, &name),
        Some(Commands::Snap { name, project }) => {
            commands::snap::run(&paths, &scope(project)?, name)
        }
        Some(Commands::Edit { name }) => commands::edit::run(&paths, &name),
        Some(Commands::Show { name, project }) => commands::show::run(&paths, &scope(project)?, name),
        Some(Commands::Diff { name, project }) => commands::diff::run(&paths, &scope(project)?, name),
        Some(Commands::Which { project }) => commands::which::run(&paths, &scope(project)?),
        Some(Commands::Skill { action }) => match action {
            SkillAction::List => commands::skill::run_list(&paths),
            SkillAction::Toggle { name } => commands::skill::run_toggle(&paths, &name),
            SkillAction::Rm { name } => commands::skill::run_rm(&paths, &name),
        },
        Some(Commands::Doctor) => commands::doctor::run(&paths),
        Some(Commands::Adopt { into, names }) => commands::adopt::run(&paths, into, names),
        Some(Commands::Migrate { copy }) => commands::migrate::run(&paths, copy),
        None => {
            // No subcommand — show global status
            commands::which::run(&paths, &Scope::Global)
        }
    }
}

/// Build the [`Scope`] for a command. `--project` resolves to the current working
/// directory (canonicalized so symlinked/relative paths map to one binding).
fn scope(project: bool) -> Result<Scope> {
    if !project {
        return Ok(Scope::Global);
    }
    let cwd = std::env::current_dir().context("Cannot determine current directory")?;
    let canonical = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    Ok(Scope::Project(canonical))
}
