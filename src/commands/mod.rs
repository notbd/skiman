//! One module per command. Each command module exposes `run(..) -> Exit`,
//! called from `cli::run`.

pub mod check;
pub mod init;
pub mod new;
pub mod sync;

use std::io::IsTerminal;

/// Whether the user can answer a prompt. Without one, flags that would
/// otherwise be asked for are required (exit 2).
pub fn has_terminal() -> bool {
    std::io::stdin().is_terminal()
}
