//! Black-box test harness shared by every integration test file.
//!
//! Use it from a test file with `mod support;` and `use support::*;`.
//! Every test file compiles this module on its own and uses only part of it.
#![allow(dead_code, unused_imports)]

mod fs;
mod pty;
mod sandbox;
mod upstream;

pub use fs::{assert_dangling_symlink, assert_symlink};
pub use pty::Pty;
pub use sandbox::{Sandbox, SandboxBuilder, Tool, ToolSetup};
pub use upstream::Upstream;
