//! skiman: keeps agent skills and global instruction files in one skill repo
//! and distributes them to every agent tool.

mod backup;
mod cli;
mod commands;
mod config;
mod discovery;
mod exit;
mod failure;
mod fetch;
mod fsutil;
mod git;
mod git_hooks;
mod import_guide;
mod import_record;
mod lockfile;
mod managing_skills;
mod manifest;
mod merge;
mod names;
mod ownership;
mod path_safety;
mod paths;
mod project;
mod repo;
mod repo_lock;
mod reported;
mod rules;
mod session_hooks;
mod skill_md;
mod snapshot;
mod source;
mod staged;
mod staging;
mod sync;
mod test_hook;
mod tools;
mod tree;

pub use exit::Exit;

/// Parses the process arguments, runs the command and returns its exit status.
pub fn run() -> Exit {
    cli::run(std::env::args_os())
}
