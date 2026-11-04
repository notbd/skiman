//! skiman: keeps agent skills and global instruction files in one skill repo
//! and distributes them to every agent tool.

pub mod cli;
pub mod commands;
pub mod config;
pub mod exit;
pub mod failure;
pub mod fsutil;
pub mod git_hooks;
pub mod import_guide;
pub mod lockfile;
pub mod managing_skills;
pub mod manifest;
pub mod names;
pub mod ownership;
pub mod paths;
pub mod repo;
pub mod repo_lock;
pub mod reported;
pub mod rules;
pub mod session_hooks;
pub mod staged;
pub mod sync;
pub mod tools;
pub mod tree;

pub use exit::Exit;

/// Parses the process arguments, runs the command and returns its exit status.
pub fn run() -> Exit {
    cli::run(std::env::args_os())
}
