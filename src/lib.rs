//! skiman: keeps agent skills and global instruction files in one skill repo
//! and distributes them to every agent tool.

pub mod cli;
pub mod commands;
pub mod exit;
pub mod paths;

pub use exit::Exit;

/// Parses the process arguments, runs the command and returns its exit status.
pub fn run() -> Exit {
    cli::run(std::env::args_os())
}
