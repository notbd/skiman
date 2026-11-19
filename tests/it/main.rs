//! Every integration test, compiled into one test binary: `tests/it/support`
//! is built once and all tests run in parallel. One module per command or
//! area.

mod support;

mod add;
mod add_sources;
mod adopt;
mod check;
mod cli;
mod cross_filesystem;
mod diff;
mod doctor;
mod groups;
mod harness;
mod hooks;
mod hostile_upstream;
mod import;
mod import_again;
mod import_guide;
mod import_resume;
mod init;
mod instructions;
mod linked;
mod list;
mod mv;
mod new;
mod overview;
mod ownership;
mod project_add;
mod project_adopt;
mod project_check;
mod project_kill;
mod project_restore;
mod project_sync;
mod remove;
mod secrets;
mod session_hooks;
mod skiman_manual;
mod sync;
mod update;
mod update_conflicts;
mod update_recovery;
mod update_refs;
mod upstream_cache;
