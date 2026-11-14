//! skiman: keeps agent skills and global instruction files in one skill repo
//! and distributes them to every agent tool.

/// `println!` that ignores write errors: when stdout is closed early
/// (`skiman add … | head`), a command still finishes its writes and returns
/// its own exit status instead of panicking between two of them.
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// `eprintln!` that ignores write errors, for the same reason as `out!`.
macro_rules! err {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

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
mod upstream;

pub use exit::Exit;

/// Parses the process arguments, runs the command and returns its exit
/// status, once what it flushed is on the disk (ADR 0005).
pub fn run() -> Exit {
    let exit = cli::run(std::env::args_os());
    match fsutil::drain() {
        Ok(()) => exit,
        Err(e) => {
            err!("error: {e}");
            exit.combine(Exit::OperationalError)
        }
    }
}
