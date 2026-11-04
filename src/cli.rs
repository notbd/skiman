//! Command-line definitions (every command's flags live here) and dispatch.

use std::ffi::OsString;

use clap::{Parser, Subcommand};

use crate::Exit;

#[derive(Debug, Parser)]
#[command(name = "skiman", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// One variant per command; each command's logic lives in `commands::<name>`.
#[derive(Debug, Subcommand)]
pub enum Command {}

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
    match cli.command {}
}
