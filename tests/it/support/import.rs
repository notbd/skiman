//! The setup most import tests start from, and the ways they run the import.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use super::fixtures::skill_md;
use super::fs::write;
use super::sandbox::{Sandbox, Tool};
use super::upstream::Upstream;

/// What [`setup`] made.
pub struct Setup {
    /// The import folder.
    pub folder: PathBuf,
    /// Upstream `acme/skills`, holding `skills/pdf`.
    pub upstream: Upstream,
    /// Its one commit, which the import folder pins `pdf` to.
    pub commit: String,
}

/// Instruction files that are regular files, a skill copied into one tool
/// directory and linked from another, a copy the import leaves out, a skill
/// repo whose shared body is `old shared body`, and an import folder holding
/// own skill `notes` and third-party skill `pdf` (pinned, with a local edit).
/// The commit has a fixed date, so every sandbox gets the same one and the
/// final states of two sandboxes compare.
pub fn setup(sandbox: &Sandbox) -> Setup {
    write(
        sandbox.instruction_file(Tool::ClaudeCode),
        "old claude rules\n",
    );
    write(sandbox.instruction_file(Tool::Codex), "old codex rules\n");
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write(claude.join("notes/SKILL.md"), skill_md("notes"));
    write(claude.join("old-copy/SKILL.md"), skill_md("old-copy"));
    std::os::unix::fs::symlink(
        claude.join("notes"),
        sandbox.tool_dir(Tool::Codex).join("notes"),
    )
    .unwrap();

    let upstream = sandbox.upstream("acme/skills");
    upstream
        .write("skills/pdf/SKILL.md", skill_md("pdf"))
        .write("skills/pdf/reference.md", "# Reference\n");
    upstream.git(["add", "--all"]);
    let committed = sandbox
        .std_command("git")
        .current_dir(upstream.path())
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.test"])
        .args(["commit", "--quiet", "-m", "v1"])
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .status()
        .unwrap();
    assert!(committed.success());
    let commit = upstream.head();

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(3);
    write(
        sandbox.skill_repo().join("instructions/shared.md"),
        "old shared body\n",
    );

    let folder = sandbox.root().join("import");
    write(folder.join("skills/notes/SKILL.md"), skill_md("notes"));
    write(
        folder.join("skills/pdf/SKILL.md"),
        format!("{}\nMy local note.\n", skill_md("pdf")),
    );
    write(folder.join("skills/pdf/reference.md"), "# Reference\n");
    write(folder.join("instructions/shared.md"), "# Shared\n");
    write(
        folder.join("skiman.toml"),
        format!(
            "[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n\n[skills.pdf.source]\nurl = \"https://github.com/acme/skills.git\"\npath = \"skills/pdf\"\nref = \"{commit}\"\n"
        ),
    );
    Setup {
        folder,
        upstream,
        commit,
    }
}

/// `skiman import <folder> --yes`.
pub fn import(sandbox: &Sandbox, folder: &Path) -> Output {
    sandbox
        .skiman()
        .arg("import")
        .arg(folder)
        .arg("--yes")
        .output()
        .unwrap()
}

/// `skiman import <folder> --yes` running the shell command `run` at
/// `hook`, a `<scope>:<point>` of the test hook (debug builds only).
pub fn import_with_hook(sandbox: &Sandbox, folder: &Path, hook: &str, run: &str) -> Output {
    sandbox
        .skiman()
        .arg("import")
        .arg(folder)
        .arg("--yes")
        .env("SKIMAN_TEST_HOOK", hook)
        .env("SKIMAN_TEST_HOOK_RUN", run)
        .output()
        .unwrap()
}

/// Runs `skiman import <folder> --yes` and kills it with signal 9 at
/// `import:<point>` of the test hook.
#[track_caller]
pub fn import_killed_at(sandbox: &Sandbox, folder: &Path, point: &str) {
    let output = import_with_hook(sandbox, folder, &format!("import:{point}"), "kill -9 $PPID");
    assert_eq!(
        output.status.signal(),
        Some(9),
        "{point}: {:?}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}
