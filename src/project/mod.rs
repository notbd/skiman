//! The project scope (`docs/project-scope.md`): project instruction files and
//! project skills under user-registered project roots, kept as real files in
//! the projects and mirrored into the skill repo under `projects/`.
//!
//! The mirror follows the project: sync copies project changes into the
//! mirror (`sync`); the project is written only by `skiman project restore`.
//! Every replace goes through [`swap::replace`], and whatever it replaces
//! goes to a backup.

pub mod content;
pub mod discover;
pub mod git;
pub mod item;
pub mod pattern;
pub mod root;
pub mod state;
pub mod swap;
pub mod sync;
pub mod synced;
