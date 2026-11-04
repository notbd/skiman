//! One module per command. Each command module exposes `run(..) -> Exit`,
//! called from `cli::run`.

pub mod add;
pub mod adopt;
pub mod check;
pub mod diff;
pub mod doctor;
pub mod import;
pub mod init;
pub mod list;
pub mod mv;
pub mod new;
pub mod remove;
pub mod sync;
pub mod update;

use std::io::{IsTerminal, Write};

use crate::failure::Failure;

/// Whether the user can answer a prompt. Without one, flags that would
/// otherwise be asked for are required (exit 2).
pub fn has_terminal() -> bool {
    std::io::stdin().is_terminal()
}

/// Asks `question` on stderr; only `y` or `yes` agrees.
pub fn confirm(question: &str) -> Result<bool, Failure> {
    let io = |e: std::io::Error| Failure::operational(format!("cannot read the answer: {e}"));
    eprint!("{question}");
    std::io::stderr().flush().map_err(io)?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(io)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}
