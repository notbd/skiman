//! Black-box test harness shared by every integration test file.
//!
//! Test modules use it with `use crate::support::{...}`.

mod fs;
mod pty;
mod sandbox;
mod tree;
mod upstream;

pub use fs::{assert_dangling_symlink, assert_symlink};
pub use pty::Pty;
pub use sandbox::{Sandbox, Tool, ToolSetup};
pub use tree::tree;
pub use upstream::Upstream;
