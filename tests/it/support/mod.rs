//! Black-box test harness shared by every integration test module.
//!
//! Test modules use it with `use crate::support::{...}`.

mod commands;
mod fixtures;
mod fs;
mod hash;
mod import;
mod pty;
mod sandbox;
mod tree;
mod upstream;

pub use commands::{backup_dirs, backups, check_is_clean, init, only_backup};
pub use fixtures::{PDF_SKILL_MD, aws_key, lines, skill_md};
pub use fs::{
    assert_dangling_symlink, assert_symlink, exists, is_executable, moved_note, read, write,
};
pub use hash::{fingerprint, hex, tree_hash};
pub use import::{Setup, import, import_killed_at, import_with_hook, setup};
pub use pty::Pty;
pub use sandbox::{Sandbox, Tool, ToolSetup};
pub use tree::tree;
pub use upstream::Upstream;
