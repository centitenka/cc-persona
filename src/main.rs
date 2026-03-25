mod active_persona;
mod backup;
mod claude;
mod cli;
mod commands;
mod config;
mod persona;
mod symlink;
#[cfg(test)]
mod test_support;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Commands, SkillAction};
use config::Paths;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::new()?;

    match cli.command {
        Some(Commands::Init) => commands::init::run(&paths),
        Some(Commands::List) => commands::list::run(&paths),
        Some(Commands::Use {
            name,
            save_current,
            discard_current,
        }) => commands::use_cmd::run(&paths, name, save_current, discard_current),
        Some(Commands::Off {
            save_current,
            discard_current,
        }) => commands::off::run(&paths, save_current, discard_current),
        Some(Commands::Create { name }) => commands::create::run(&paths, &name),
        Some(Commands::Snap { name }) => commands::snap::run(&paths, name),
        Some(Commands::Edit { name }) => commands::edit::run(&paths, &name),
        Some(Commands::Show { name }) => commands::show::run(&paths, name),
        Some(Commands::Diff { name }) => commands::diff::run(&paths, name),
        Some(Commands::Which) => commands::which::run(&paths),
        Some(Commands::Skill { action }) => match action {
            SkillAction::List => commands::skill::run_list(&paths),
            SkillAction::Toggle { name } => commands::skill::run_toggle(&paths, &name),
            SkillAction::Rm { name } => commands::skill::run_rm(&paths, &name),
        },
        None => {
            // No subcommand — show status
            commands::which::run(&paths)
        }
    }
}
