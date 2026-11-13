//! Command-line definitions (every command's flags live here) and dispatch.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

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
    Add(AddArgs),
    /// Show the local modifications of a third-party skill against its upstream snapshot
    Diff {
        /// Name of the third-party skill
        name: String,
    },
    /// Update third-party skills, merging upstream changes with local modifications
    Update {
        /// Skills to update; without names, every third-party skill
        names: Vec<String>,
        /// Move the skill to this branch, tag or full 40-hex commit (single skill only)
        #[arg(long = "ref", value_name = "REF")]
        git_ref: Option<String>,
        /// Show the new versions, changed files and conflicts without changing the skill repo or
        /// the tool directories (exit 0 even with conflicts)
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a skill: its folders go to a backup, its records and links are removed
    Remove {
        /// Name of the skill
        name: String,
        /// Remove without asking for confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Rename a skill: folder, `name` field, records, upstream snapshot and secret exemptions
    Mv {
        /// Current name
        old: String,
        /// New name
        new: String,
    },
    /// Bring a foreign skill from an agent tool's tool directory into the skill repo
    Adopt {
        /// Name of the entry in the tool directory, which becomes the skill's name
        name: String,
        /// Id of the agent tool whose tool directory holds the entry
        #[arg(long, value_name = "tool id", value_parser = parse_tool_id)]
        from: String,
        /// Adopt without asking for confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Import a prepared folder
    Import {
        /// The import folder: skills/, instructions/shared.md and skiman.toml [default: <skill repo>/.skiman/import/]
        #[arg(conflicts_with = "guide")]
        folder: Option<PathBuf>,
        /// Import without asking for confirmation
        #[arg(long, conflicts_with = "guide")]
        yes: bool,
        /// Print the instructions an agent follows to prepare an import folder
        #[arg(long)]
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
    /// Manage project instruction files and project skills under registered project roots
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
}

#[derive(Debug, Clone, Args)]
pub struct AddArgs {
    /// Where the skills come from: <owner>/<repo>, an https:// or ssh:// git URL,
    /// user@host:path, a GitHub tree or blob URL, or a local folder (/, ~ or .)
    pub source: String,
    /// Install the skill with this upstream name, or the one in this folder when the value
    /// holds a `/` (`pdf/` for a folder at the top; repeatable)
    #[arg(long = "skill", value_name = "NAME|FOLDER/")]
    pub skills: Vec<String>,
    /// Install every skill the source holds
    #[arg(long, conflicts_with = "skills")]
    pub all: bool,
    /// Install name, when it must differ from the upstream name (single skill only)
    #[arg(long)]
    pub name: Option<String>,
    /// Branch, tag or full 40-hex commit to install instead of the default branch
    #[arg(long = "ref", value_name = "REF")]
    pub git_ref: Option<String>,
    /// Show the file list, then stop without writing anything
    #[arg(long)]
    pub dry_run: bool,
    /// Install without asking for confirmation
    #[arg(long)]
    pub yes: bool,
}

/// `skiman project <command>`; the logic lives in `commands::project`.
#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// Register a folder as a project root
    Add {
        /// The folder; its project files are discovered by `skiman project adopt`
        path: PathBuf,
        /// Root id, used as the mirror folder name under projects/ [default: the folder name]
        #[arg(long)]
        id: Option<String>,
    },
    /// Copy project items into the mirror: new ones under a folder, any one named exactly
    Adopt {
        /// Folders or items to adopt [default: every project root]
        paths: Vec<PathBuf>,
        /// Print the plan, then stop without writing anything
        #[arg(long)]
        dry_run: bool,
        /// Adopt without asking for confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Write mirror items to the project: missing ones under a folder, any one named exactly
    Restore {
        /// Folders or items to restore
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Print the plan, then stop without writing anything
        #[arg(long)]
        dry_run: bool,
        /// Restore without asking for confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Stop managing project items: their mirrors go to a backup, the project is untouched
    Eject {
        /// Folders or items; a project root's own path also unregisters it
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Eject without asking for confirmation
        #[arg(long)]
        yes: bool,
    },
}

/// The id of a known agent tool.
fn parse_tool_id(value: &str) -> Result<String, String> {
    if TOOLS.iter().any(|tool| tool.id == value) {
        return Ok(value.to_owned());
    }
    let ids: Vec<&str> = TOOLS.iter().map(|tool| tool.id).collect();
    Err(format!(
        "`{value}` is not an agent tool id (known: {})",
        ids.join(", ")
    ))
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
        Command::Add(args) => commands::add::run(args),
        Command::Diff { name } => commands::diff::run(&name),
        Command::Update {
            names,
            git_ref,
            dry_run,
        } => commands::update::run(names, git_ref, dry_run),
        Command::Remove { name, yes } => commands::remove::run(&name, yes),
        Command::Mv { old, new } => commands::mv::run(&old, &new),
        Command::Adopt { name, from, yes } => commands::adopt::run(&name, &from, yes),
        Command::Import { folder, yes, guide } => {
            if guide {
                commands::import::guide()
            } else {
                commands::import::run(folder.as_deref(), yes)
            }
        }
        Command::Sync { hook } => commands::sync::run(hook),
        Command::Check { staged } => commands::check::run(staged),
        Command::List => commands::list::run(),
        Command::Doctor => commands::doctor::run(),
        Command::Project { command } => match command {
            ProjectCommand::Add { path, id } => commands::project::add(&path, id),
            ProjectCommand::Adopt {
                paths,
                dry_run,
                yes,
            } => commands::project::adopt(paths, dry_run, yes),
            ProjectCommand::Restore {
                paths,
                dry_run,
                yes,
            } => commands::project::restore(paths, dry_run, yes),
            ProjectCommand::Eject { paths, yes } => commands::project::eject(paths, yes),
        },
    }
}
