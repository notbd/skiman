//! The project scope (`docs/project-scope.md`): project instruction files and
//! project skills under user-registered project roots, kept as real files in
//! the projects and mirrored into the canon under `projects/`.
//!
//! The mirror follows the project: sync copies project changes into the
//! mirror (`sync`); the project is written only by `skiman project restore`.
//! Every replace goes through [`swap::Replace`], and whatever it replaces
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

use std::path::PathBuf;

/// One paste-ready `skiman project <verb>` command line naming `paths`,
/// each quoted for the shell when it needs it.
pub fn command(verb: &str, paths: &[PathBuf]) -> String {
    let quoted: Vec<String> = paths
        .iter()
        .map(|p| shell_quote(&p.display().to_string()))
        .collect();
    format!("skiman project {verb} {}", quoted.join(" "))
}

/// What the canon is in the middle of (`the canon <op>`), when a git
/// operation runs there: project sync pauses, project commands refuse.
pub fn operation(repo: &crate::canon::Canon) -> Option<String> {
    git::operation_in_progress(repo.root()).map(|op| format!("the canon {op}"))
}

/// Why no project item can be changed now: a linked folder cannot be read,
/// so which items overlap it is unknown, or [`operation`]. `adopt`,
/// `restore` and doctor ask.
pub fn paused_by(repo: &crate::canon::Canon, linked: &crate::linked::Linked) -> Option<String> {
    let unreadable = linked.unreadable().map(|folder| {
        format!(
            "linked folder `{}` cannot be read, so which project items overlap it is unknown",
            folder.id
        )
    });
    unreadable.or_else(|| operation(repo))
}

pub fn shell_quote(text: &str) -> String {
    let plain = |b: u8| !b.is_ascii() || b.is_ascii_alphanumeric() || b"/._-~+,@%=".contains(&b);
    if !text.is_empty() && text.bytes().all(plain) {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_quote_what_the_shell_would_split() {
        let paths = [
            PathBuf::from("/w/a b/AGENTS.md"),
            PathBuf::from("/w/中文/AGENTS.md"),
        ];
        assert_eq!(
            command("restore", &paths),
            "skiman project restore '/w/a b/AGENTS.md' /w/中文/AGENTS.md"
        );
        assert_eq!(
            command("adopt", &[PathBuf::from("/w/it's")]),
            r"skiman project adopt '/w/it'\''s'"
        );
    }
}
