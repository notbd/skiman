//! Running `skiman import <folder>` on a skill repo that already holds what
//! the import writes (an earlier run finished, or stopped during the
//! takeover): nothing is fetched or written to the skill repo, and only the
//! originals still in place move. A shared body that differs is refused,
//! with a way on for an import that ran before and one for an import that
//! never did.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use crate::support::{
    Sandbox, Setup, Tool, backup_dirs, import, import_killed_at, init, read, setup, skill_md, tree,
    write,
};

/// The preview line when the skill repo already holds the import.
const HOLDS: &str = "  the skill repo already holds everything this import writes: nothing is written to it, and its skiman.toml stays as it is\n";

const TOOLS: &str = "[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n";

/// The skill repo and the home folder without the git cache (fetching may
/// fill it), backups included.
fn world(sandbox: &Sandbox) -> Vec<BTreeMap<PathBuf, String>> {
    let mut home = tree(sandbox.home());
    home.retain(|path, _| !path.starts_with(".local/state/skiman/cache"));
    vec![tree(sandbox.skill_repo()), home]
}

/// Runs the import, which must exit with `code`; returns its stdout.
#[track_caller]
fn run(sandbox: &Sandbox, folder: &Path, code: i32) -> String {
    let Output {
        status,
        stdout,
        stderr,
    } = import(sandbox, folder);
    let stdout = String::from_utf8(stdout).unwrap();
    let stderr = String::from_utf8(stderr).unwrap();
    assert_eq!(status.code(), Some(code), "{stdout}\n{stderr}");
    stdout
}

/// Points the folder's `pdf` source at branch `main` instead of `commit`.
fn follow_main(folder: &Path, commit: &str) {
    let manifest = folder.join("skiman.toml");
    write(&manifest, read(&manifest).replace(commit, "main"));
}

/// The refusal when everything but the shared body is in place: exit 1,
/// both ways on, nothing changed.
#[track_caller]
fn assert_shared_body_refused(sandbox: &Sandbox, folder: &Path) {
    let before = world(sandbox);
    let folder_before = tree(folder);
    let output = import(sandbox, folder);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    for text in [
        "already holds everything this import writes except instructions/shared.md",
        "If this import has run on this skill repo before",
        "If it has never run on this skill repo",
        "If you cannot tell which, do the first.",
    ] {
        assert!(stderr.contains(text), "{text:?} not in {stderr}");
    }
    assert!(!stderr.contains("already finished"), "{stderr}");
    assert_eq!(world(sandbox), before, "{stderr}");
    assert_eq!(tree(folder), folder_before);
}

#[test]
fn an_import_run_again_after_it_finished_fetches_nothing_and_writes_nothing() {
    let sandbox = Sandbox::new();
    let Setup {
        folder,
        upstream,
        commit,
    } = setup(&sandbox);
    follow_main(&folder, &commit);
    run(&sandbox, &folder, 0);
    let before = world(&sandbox);

    // The branch moved on: the snapshot and skiman.lock stay as they are.
    upstream.write("skills/pdf/reference.md", "# Reference v2\n");
    upstream.commit("v2");
    let stdout = run(&sandbox, &folder, 0);
    assert!(stdout.contains(HOLDS), "{stdout}");
    assert_eq!(world(&sandbox), before);

    // Without any upstream, it still finishes.
    std::fs::remove_dir_all(sandbox.upstreams_dir()).unwrap();
    run(&sandbox, &folder, 0);
    assert_eq!(world(&sandbox), before);
}

#[test]
fn a_blank_or_missing_shared_body_in_the_skill_repo_is_the_same_as_an_empty_one_in_the_import_folder()
 {
    let sandbox = Sandbox::new();
    let Setup {
        folder,
        upstream,
        commit,
    } = setup(&sandbox);
    follow_main(&folder, &commit);
    write(folder.join("instructions/shared.md"), "");
    run(&sandbox, &folder, 0);
    let repo = sandbox.skill_repo();
    let snapshot = tree(repo.join("upstream/pdf"));
    upstream.write("skills/pdf/reference.md", "# Reference v2\n");
    upstream.commit("v2");

    write(repo.join("instructions/shared.md"), "\n");
    let before = tree(&repo);
    let stdout = run(&sandbox, &folder, 0);
    assert!(stdout.contains(HOLDS), "{stdout}");
    assert_eq!(tree(&repo), before);

    std::fs::remove_file(repo.join("instructions/shared.md")).unwrap();
    let before = tree(&repo);
    let stdout = run(&sandbox, &folder, 0);
    assert!(stdout.contains(HOLDS), "{stdout}");
    assert_eq!(tree(&repo), before);
    assert_eq!(tree(repo.join("upstream/pdf")), snapshot);
}

#[test]
fn project_roots_and_enabled_set_after_an_import_are_kept_when_it_runs_again() {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    run(&sandbox, &folder, 0);
    write(sandbox.home().join("proj/AGENTS.md"), "# Proj\n");
    sandbox
        .skiman()
        .args(["project", "add"])
        .arg(sandbox.home().join("proj"))
        .assert()
        .code(0);
    let repo = sandbox.skill_repo();
    let manifest = read(repo.join("skiman.toml"));
    let backups = backup_dirs(&sandbox);

    let stdout = run(&sandbox, &folder, 0);
    assert!(stdout.contains(HOLDS), "{stdout}");
    assert!(
        !stdout.contains("differs from the skill repo's"),
        "{stdout}"
    );
    assert_eq!(read(repo.join("skiman.toml")), manifest);
    assert_eq!(backup_dirs(&sandbox), backups);

    // The same tools in another order are no difference.
    write(
        folder.join("skiman.toml"),
        read(folder.join("skiman.toml")).replace(
            "[\"claude-code\", \"codex\", \"antigravity\"]",
            "[\"antigravity\", \"codex\", \"claude-code\"]",
        ),
    );
    let stdout = run(&sandbox, &folder, 0);
    assert!(
        !stdout.contains("differs from the skill repo's"),
        "{stdout}"
    );

    // A skill turned off in the skill repo stays off, and the preview says so.
    let manifest = format!("{manifest}\n[skills.notes]\nenabled = false\n");
    write(repo.join("skiman.toml"), &manifest);
    let stdout = run(&sandbox, &folder, 0);
    assert!(
        stdout.contains(
            "  the import folder's skiman.toml differs from the skill repo's in [skills.notes] enabled; the skill repo's are kept\n"
        ),
        "{stdout}"
    );
    assert_eq!(read(repo.join("skiman.toml")), manifest);
}

/// An import folder holding own skill `notes` only, with `tools`.
fn notes_only(folder: &Path, tools: &str) {
    std::fs::remove_dir_all(folder.join("skills/pdf")).unwrap();
    write(folder.join("skiman.toml"), tools);
}

#[test]
fn a_shared_body_changed_after_an_import_refuses_running_it_again() {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    run(&sandbox, &folder, 0);
    write(
        sandbox.skill_repo().join("instructions/shared.md"),
        "# Shared\n\nEdited later.\n",
    );
    assert_shared_body_refused(&sandbox, &folder);

    // An import without skills too.
    let sandbox = Sandbox::new();
    init(&sandbox);
    let folder = sandbox.root().join("import");
    std::fs::create_dir_all(folder.join("skills")).unwrap();
    write(folder.join("instructions/shared.md"), "# Shared\n");
    write(folder.join("skiman.toml"), TOOLS);
    run(&sandbox, &folder, 0);
    let shared = sandbox.skill_repo().join("instructions/shared.md");
    assert_eq!(read(&shared), "# Shared\n");
    write(&shared, "# Shared\n\nEdited later.\n");
    assert_shared_body_refused(&sandbox, &folder);
}

/// Which copy of the two shared bodies is made the same as the other.
#[derive(Clone, Copy, Debug)]
enum Keep {
    /// The skill repo's is copied over the import folder's.
    Repo,
    /// The import folder's is copied over the skill repo's.
    Folder,
}

/// Copies one shared body over the other as `keep` says; returns the one kept.
fn copy_shared_body(sandbox: &Sandbox, folder: &Path, keep: Keep) -> String {
    let ours = sandbox.skill_repo().join("instructions/shared.md");
    let theirs = folder.join("instructions/shared.md");
    let (from, to) = match keep {
        Keep::Repo => (ours, theirs),
        Keep::Folder => (theirs, ours),
    };
    std::fs::copy(&from, to).unwrap();
    read(from)
}

/// After the refusal, either copy lets the same command finish the takeover:
/// the Codex instruction file is taken over, and the shared body is the one kept.
#[track_caller]
fn assert_either_copy_finishes(start: impl Fn(&Sandbox) -> PathBuf) {
    for keep in [Keep::Repo, Keep::Folder] {
        let sandbox = Sandbox::new();
        let folder = start(&sandbox);
        assert_shared_body_refused(&sandbox, &folder);
        let kept = copy_shared_body(&sandbox, &folder, keep);

        let stdout = run(&sandbox, &folder, 0);
        assert!(stdout.contains(HOLDS), "{keep:?}: {stdout}");
        let shared = sandbox.skill_repo().join("instructions/shared.md");
        assert_eq!(read(&shared), kept, "{keep:?}");
        crate::support::assert_symlink(sandbox.instruction_file(Tool::Codex), &shared, &shared);
        assert!(
            backup_dirs(&sandbox)
                .iter()
                .any(|dir| dir.join("codex/AGENTS.md").is_file()),
            "{keep:?}: the Codex instruction file is in no backup"
        );
    }
}

#[test]
fn after_a_kill_during_the_takeover_and_an_edit_of_the_shared_body_either_copy_finishes_the_import()
{
    assert_either_copy_finishes(|sandbox| {
        let Setup { folder, .. } = setup(sandbox);
        import_killed_at(sandbox, &folder, "moved:claude-code/CLAUDE.md");
        sandbox.skiman().arg("sync").output().unwrap();
        // A session writes to the shared body through the instruction file's link.
        let claude_md = sandbox.instruction_file(Tool::ClaudeCode);
        write(&claude_md, format!("{}Remember this.\n", read(&claude_md)));
        folder
    });
}

#[test]
fn after_adopt_an_import_holding_the_same_skill_writes_the_import_folders_shared_body_and_skiman_toml()
 {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    let repo = sandbox.skill_repo();
    write(repo.join("instructions/shared.md"), "");
    sandbox
        .skiman()
        .args(["adopt", "notes", "--from", "claude-code", "--yes"])
        .output()
        .unwrap();
    assert_eq!(read(repo.join("skills/notes/SKILL.md")), skill_md("notes"));
    notes_only(&folder, TOOLS);

    run(&sandbox, &folder, 0);
    assert_eq!(read(repo.join("instructions/shared.md")), "# Shared\n");
    assert_eq!(read(repo.join("skiman.toml")), TOOLS);
    assert!(
        backup_dirs(&sandbox).iter().any(|dir| {
            dir.join("claude-code/CLAUDE.md").is_file()
                && read(dir.join("claude-code/CLAUDE.md")) == "old claude rules\n"
        }),
        "the Claude Code instruction file is in no backup"
    );
}

/// A skill repo where `skiman new mine` made the only skill and whose shared
/// body holds something, and an import folder holding the same skill, a
/// shared body of its own and other tools.
fn never_imported(sandbox: &Sandbox) -> PathBuf {
    let Setup { folder, .. } = setup(sandbox);
    sandbox
        .skiman()
        .args(["new", "mine", "-d", "My skill"])
        .output()
        .unwrap();
    std::fs::remove_dir_all(folder.join("skills")).unwrap();
    std::fs::create_dir_all(folder.join("skills")).unwrap();
    let copied = sandbox
        .std_command("cp")
        .arg("-R")
        .arg(sandbox.skill_repo().join("skills/mine"))
        .arg(folder.join("skills/mine"))
        .status()
        .unwrap();
    assert!(copied.success());
    write(
        folder.join("instructions/shared.md"),
        "Rules from my CLAUDE.md.\n",
    );
    write(
        folder.join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\", \"codex\"]\n",
    );
    folder
}

#[test]
fn an_import_that_never_ran_imports_everything_once_the_skill_repos_shared_body_is_emptied() {
    for empty in ["", "\n"] {
        let sandbox = Sandbox::new();
        let folder = never_imported(&sandbox);
        assert_shared_body_refused(&sandbox, &folder);
        let repo = sandbox.skill_repo();
        write(repo.join("instructions/shared.md"), empty);

        run(&sandbox, &folder, 0);
        assert_eq!(
            read(repo.join("instructions/shared.md")),
            "Rules from my CLAUDE.md.\n"
        );
        assert_eq!(
            read(repo.join("skiman.toml")),
            read(folder.join("skiman.toml"))
        );
        assert!(
            backup_dirs(&sandbox)
                .iter()
                .any(|dir| dir.join("codex/AGENTS.md").is_file()),
            "{empty:?}: the Codex instruction file is in no backup"
        );
    }

    // Copying instead keeps the skill repo's skiman.toml and names the tools.
    let sandbox = Sandbox::new();
    let folder = never_imported(&sandbox);
    let repo = sandbox.skill_repo();
    copy_shared_body(&sandbox, &folder, Keep::Folder);
    // The skill repo apart from the command's own staging folder.
    let content = || {
        let mut content = tree(&repo);
        content.retain(|path, _| !path.starts_with(".skiman/tmp"));
        content
    };
    let before = content();
    let stdout = run(&sandbox, &folder, 0);
    assert!(stdout.contains(HOLDS), "{stdout}");
    assert!(
        stdout.contains(
            "  the import folder's skiman.toml differs from the skill repo's in [tools] enabled; the skill repo's are kept\n"
        ),
        "{stdout}"
    );
    assert_eq!(content(), before);
}

#[test]
fn after_a_kill_during_the_takeover_either_copy_finishes_without_reaching_the_upstream() {
    // A local folder upstream that is gone.
    assert_either_copy_finishes(|sandbox| {
        let Setup { folder, .. } = setup(sandbox);
        let source = sandbox.home().join("src/pdf");
        write(source.join("SKILL.md"), skill_md("pdf"));
        write(source.join("reference.md"), "# Reference\n");
        write(
            folder.join("skiman.toml"),
            format!("{TOOLS}\n[skills.pdf.source]\nurl = \"~/src/pdf\"\npath = \"\"\n"),
        );
        import_killed_at(sandbox, &folder, "moved:claude-code/notes");
        std::fs::remove_dir_all(&source).unwrap();
        write(folder.join("instructions/shared.md"), "# Shared v2\n");
        folder
    });
    // A branch whose upstream cannot be reached.
    assert_either_copy_finishes(|sandbox| {
        let Setup { folder, commit, .. } = setup(sandbox);
        follow_main(&folder, &commit);
        import_killed_at(sandbox, &folder, "moved:claude-code/notes");
        std::fs::remove_dir_all(sandbox.upstreams_dir()).unwrap();
        write(folder.join("instructions/shared.md"), "# Shared v2\n");
        folder
    });
}

/// Runs the import and asserts it refuses (exit 1) naming `upstream/pdf`
/// and the `skiman.lock` entry of `pdf`, changing nothing.
#[track_caller]
fn assert_pdf_records_refused(sandbox: &Sandbox, folder: &Path) {
    let before = world(sandbox);
    let output = import(sandbox, folder);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    for text in [
        "  upstream/pdf is the snapshot of no third-party skill in the import folder: remove it from the skill repo, or put pdf in the import folder as a third-party skill\n",
        "  skiman.lock has an entry for pdf, which is no third-party skill in the import folder: remove the entry from the skill repo's skiman.lock, or put pdf in the import folder as a third-party skill\n",
    ] {
        assert!(stderr.contains(text), "{text:?} not in {stderr}");
    }
    assert_eq!(world(sandbox), before, "{stderr}");
}

#[test]
fn a_skill_repo_whose_third_party_records_the_import_folder_no_longer_declares_refuses_the_import()
{
    // Stopped before the last skill; the folder moved the source to notes.
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    import_killed_at(&sandbox, &folder, "rename:skills/pdf");
    std::fs::remove_dir_all(folder.join("skills/pdf")).unwrap();
    let manifest = folder.join("skiman.toml");
    write(
        &manifest,
        read(&manifest).replace("[skills.pdf.source]", "[skills.notes.source]"),
    );
    assert_pdf_records_refused(&sandbox, &folder);

    // The same stop; the folder dropped pdf and its source.
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    import_killed_at(&sandbox, &folder, "rename:skills/pdf");
    notes_only(&folder, TOOLS);
    assert_pdf_records_refused(&sandbox, &folder);

    // Finished; the folder dropped the source of pdf, which became an own skill.
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    run(&sandbox, &folder, 0);
    write(folder.join("skiman.toml"), TOOLS);
    assert_pdf_records_refused(&sandbox, &folder);
}

#[test]
fn a_snapshot_or_lock_entry_deleted_after_an_import_is_written_again_when_it_runs_again() {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    run(&sandbox, &folder, 0);
    let repo = sandbox.skill_repo();
    let snapshot = tree(repo.join("upstream/pdf"));
    let lock = read(repo.join("skiman.lock"));

    std::fs::remove_dir_all(repo.join("upstream/pdf")).unwrap();
    run(&sandbox, &folder, 0);
    assert_eq!(tree(repo.join("upstream/pdf")), snapshot);

    write(repo.join("skiman.lock"), "version = 1\n");
    run(&sandbox, &folder, 0);
    assert_eq!(read(repo.join("skiman.lock")), lock);
}

#[test]
fn a_skill_removed_after_an_import_comes_back_with_the_folders_shared_body_and_the_project_roots_kept()
 {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    run(&sandbox, &folder, 0);
    sandbox
        .skiman()
        .args(["remove", "pdf", "--yes"])
        .assert()
        .code(0);
    let repo = sandbox.skill_repo();
    write(
        repo.join("instructions/shared.md"),
        "Edited after the import.\n",
    );
    write(sandbox.home().join("proj/AGENTS.md"), "# Proj\n");
    sandbox
        .skiman()
        .args(["project", "add"])
        .arg(sandbox.home().join("proj"))
        .assert()
        .code(0);

    run(&sandbox, &folder, 0);
    assert_eq!(
        read(repo.join("skills/pdf/SKILL.md")),
        read(folder.join("skills/pdf/SKILL.md"))
    );
    assert_eq!(read(repo.join("instructions/shared.md")), "# Shared\n");
    assert!(
        backup_dirs(&sandbox).iter().any(|dir| {
            dir.join("repo/instructions/shared.md").is_file()
                && read(dir.join("repo/instructions/shared.md")) == "Edited after the import.\n"
        }),
        "the edited shared body is in no backup"
    );
    let manifest = read(repo.join("skiman.toml"));
    assert!(
        manifest.contains("[projects.proj]\npath = \"~/proj\"\n"),
        "{manifest}"
    );
}

#[test]
fn a_ref_changed_by_update_after_an_import_is_kept_when_it_runs_again() {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    run(&sandbox, &folder, 0);
    sandbox
        .skiman()
        .args(["update", "pdf", "--ref", "main"])
        .assert()
        .code(0);
    let repo = sandbox.skill_repo();
    let manifest = read(repo.join("skiman.toml"));
    assert!(manifest.contains("ref = \"main\""), "{manifest}");

    let stdout = run(&sandbox, &folder, 0);
    assert!(stdout.contains(HOLDS), "{stdout}");
    assert!(
        stdout.contains(
            "  the import folder's skiman.toml differs from the skill repo's in [skills.pdf.source] ref; the skill repo's are kept\n"
        ),
        "{stdout}"
    );
    assert_eq!(read(repo.join("skiman.toml")), manifest);

    write(repo.join("instructions/shared.md"), "Edited later.\n");
    assert_shared_body_refused(&sandbox, &folder);
    assert_eq!(read(repo.join("skiman.toml")), manifest);
    assert_eq!(read(repo.join("instructions/shared.md")), "Edited later.\n");
}

#[test]
fn a_source_url_or_path_changed_in_the_import_folder_after_an_import_makes_it_a_full_import_again()
{
    for (from, to) in [
        ("url = \"~/src\"", "url = \"~/src2\""),
        ("path = \"pdf\"", "path = \"pdf2\""),
    ] {
        let sandbox = Sandbox::new();
        let Setup { folder, .. } = setup(&sandbox);
        let home = sandbox.home();
        for dir in ["src/pdf", "src/pdf2", "src2/pdf"] {
            write(home.join(dir).join("SKILL.md"), skill_md("pdf"));
            write(home.join(dir).join("reference.md"), "# Reference\n");
        }
        write(home.join("src/pdf2/reference.md"), "# Moved\n");
        write(home.join("src2/pdf/reference.md"), "# Moved\n");
        let manifest = folder.join("skiman.toml");
        write(
            &manifest,
            format!("{TOOLS}\n[skills.pdf.source]\nurl = \"~/src\"\npath = \"pdf\"\n"),
        );
        run(&sandbox, &folder, 0);

        write(&manifest, read(&manifest).replace(from, to));
        let stdout = run(&sandbox, &folder, 0);
        assert!(!stdout.contains(HOLDS), "{to}: {stdout}");
        let repo = sandbox.skill_repo();
        assert_eq!(
            read(repo.join("upstream/pdf/reference.md")),
            "# Moved\n",
            "{to}"
        );
        assert!(read(repo.join("skiman.toml")).contains(to), "{to}");
    }
}
