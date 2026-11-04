//! Command-line definitions (every command's flags live here) and dispatch.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::Exit;
use crate::commands;
use crate::tools::TOOLS;

#[derive(Debug, Parser)]
#[command(name = "skiman", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// One variant per command; each command's logic lives in `commands::<name>`.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create or reuse a skill repo and distribute it
    Init {
        /// Folder of the skill repo: missing, empty, or holding skiman.toml
        path: PathBuf,
        /// Agent tools whose session-start hooks to install (comma-separated ids), or `none`
        #[arg(long, value_name = "ids|none", value_parser = parse_session_hooks)]
        session_hooks: Option<String>,
    },
    /// Create an own skill
    New {
        /// Skill name: lowercase letters, digits and single hyphens
        name: String,
        /// What the skill does and when to use it
        #[arg(short = 'd', long)]
        description: Option<String>,
    },
    /// Install a third-party skill
    Add {
        /// Where the skills come from: <owner>/<repo>, an https:// or ssh:// git URL,
        /// user@host:path, a GitHub tree or blob URL, or a local folder (/, ~ or .)
        source: String,
        /// Install the skill with this upstream name (repeatable)
        #[arg(long = "skill", value_name = "NAME")]
        skills: Vec<String>,
        /// Install every skill the source holds
        #[arg(long, conflicts_with = "skills")]
        all: bool,
        /// Install name, when it must differ from the upstream name (single skill only)
        #[arg(long)]
        name: Option<String>,
        /// Branch, tag or full 40-hex commit to install instead of the default branch
        #[arg(long = "ref", value_name = "REF")]
        git_ref: Option<String>,
        /// Show the file list, then stop without writing anything
        #[arg(long)]
        dry_run: bool,
        /// Install without asking for confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Show the local modifications of a third-party skill against its upstream snapshot
    Diff {
        /// Name of the third-party skill
        name: String,
    },
    /// Update third-party skills, merging upstream changes with local modifications
    Update {
        /// Skills to update; without names, every third-party skill
        names: Vec<String>,
    },
    /// Import a prepared folder
    Import {
        /// Print the instructions an agent follows to prepare an import folder
        #[arg(long, required = true)]
        guide: bool,
    },
    /// Make every tool directory match the skill repo
    Sync {
        /// Run from a git or session-start hook: print only new findings, always exit 0
        #[arg(long)]
        hook: bool,
    },
    /// Report every rule violation in the skill repo's working tree
    Check {
        /// Check the staged snapshot (the git index) instead of the working tree
        #[arg(long)]
        staged: bool,
    },
    /// Show every skill: kind, source, version, local modifications, conflicts, enabled
    List,
    /// Diagnose the setup: skill repo, hooks, agent tools, entries skiman does not own, leftovers
    Doctor,
}

/// `none`, or a comma-separated list of tool ids that have a session-start hook.
fn parse_session_hooks(value: &str) -> Result<String, String> {
    if value == "none" {
        return Ok(value.to_owned());
    }
    let hookable: Vec<&str> = TOOLS
        .iter()
        .filter(|tool| tool.session_hook_file.is_some())
        .map(|tool| tool.id)
        .collect();
    for id in value.split(',') {
        if !hookable.contains(&id) {
            return Err(format!(
                "`{id}` is not an agent tool with session-start hooks (use `none` or a comma-separated list of: {})",
                hookable.join(", ")
            ));
        }
    }
    Ok(value.to_owned())
}

/// Parses `args` (including the program name) and runs the command.
/// Help and version go to stdout with exit 0; usage errors go to stderr with exit 2.
pub fn run<I, T>(args: I) -> Exit
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => {
            let _ = err.print();
            return if err.use_stderr() {
                Exit::Usage
            } else {
                Exit::Success
            };
        }
    };
    match cli.command {
        Command::Init {
            path,
            session_hooks,
        } => commands::init::run(&path, session_hooks.as_deref()),
        Command::New { name, description } => commands::new::run(&name, description),
        Command::Add {
            source,
            skills,
            all,
            name,
            git_ref,
            dry_run,
            yes,
        } => commands::add::run(commands::add::Args {
            source,
            skills,
            all,
            name,
            git_ref,
            dry_run,
            yes,
        }),
        Command::Diff { name } => commands::diff::run(&name),
        Command::Update { names } => commands::update::run(names),
        Command::Import { guide: _ } => commands::import::guide(),
        Command::Sync { hook } => commands::sync::run(hook),
        Command::Check { staged } => commands::check::run(staged),
        Command::List => commands::list::run(),
        Command::Doctor => commands::doctor::run(),
    }
}
