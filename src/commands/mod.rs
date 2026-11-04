//! One module per command. Each command module exposes `run(..) -> Exit`,
//! called from `cli::run`.

pub mod add;
pub mod check;
pub mod diff;
pub mod doctor;
pub mod import;
pub mod init;
pub mod list;
pub mod new;
pub mod sync;
pub mod update;

use std::io::IsTerminal;

/// Whether the user can answer a prompt. Without one, flags that would
/// otherwise be asked for are required (exit 2).
pub fn has_terminal() -> bool {
    std::io::stdin().is_terminal()
}
