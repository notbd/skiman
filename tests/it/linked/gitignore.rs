//! The `/linked/` line of the canon's `.gitignore` (spec "`.gitignore`")
//! and the working-tree warning of `skiman check`.

use super::{COMPANY, append_manifest, folder};
use crate::support::{Sandbox, init, read};

fn linked_add(sandbox: &Sandbox, rel: &str) -> snapbox::cmd::OutputAssert {
    sandbox
        .skiman()
        .args(["linked", "add"])
        .arg(sandbox.home().join(rel))
        .assert()
}

/// A canon made before linked folders has no `/linked/` line and no
/// `linked` folder: `linked add` appends the line before any record, once.
#[test]
fn linked_add_appends_the_line_once_to_a_canon_without_it() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let gitignore = sandbox.canon().join(".gitignore");
    std::fs::write(&gitignore, ".skiman/").unwrap();
    folder(&sandbox, "work/company", &["alpha"]);
    folder(&sandbox, "work/tools", &["beta"]);

    linked_add(&sandbox, "work/company").code(0);
    assert_eq!(read(&gitignore), ".skiman/\n/linked/\n");
    linked_add(&sandbox, "work/company").code(0);
    linked_add(&sandbox, "work/tools").code(0);
    assert_eq!(read(&gitignore), ".skiman/\n/linked/\n");
    assert_eq!(
        sandbox.git(sandbox.canon(), ["status", "--porcelain", "--", "linked"]),
        ""
    );
}

/// In a canon that is not a git repo nothing can be asked of git: the line
/// is appended unless some line already is `/linked/`.
#[test]
fn in_a_canon_that_is_not_a_git_repo_the_line_is_added_unless_one_is_there() {
    let sandbox = Sandbox::new();
    let repo = sandbox.canon();
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\"]\n",
    )
    .unwrap();
    std::fs::write(repo.join(".gitignore"), "/linked/\nbuild/\n").unwrap();
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&repo)
        .assert()
        .code(0);
    assert_eq!(
        read(repo.join(".gitignore")),
        "/linked/\nbuild/\n.skiman/\n"
    );
    folder(&sandbox, "work/company", &["alpha"]);
    linked_add(&sandbox, "work/company").code(0);
    assert_eq!(
        read(repo.join(".gitignore")),
        "/linked/\nbuild/\n.skiman/\n"
    );

    std::fs::write(repo.join(".gitignore"), ".skiman/\n").unwrap();
    linked_add(&sandbox, "work/company").code(0);
    assert_eq!(read(repo.join(".gitignore")), ".skiman/\n/linked/\n");
}

/// A `.gitignore` that is a link (a canon managed with dotfiles) is never
/// written through by `linked add`: it refuses before any record.
#[test]
fn a_gitignore_that_is_a_link_stops_linked_add_with_nothing_written() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    let real = sandbox.root().join("dotfiles/gitignore");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, ".skiman/\n").unwrap();
    std::fs::remove_file(repo.join(".gitignore")).unwrap();
    std::os::unix::fs::symlink(&real, repo.join(".gitignore")).unwrap();
    folder(&sandbox, "work/company", &["alpha"]);
    let manifest = read(repo.join("skiman.toml"));

    linked_add(&sandbox, "work/company")
        .code(1)
        .stderr_eq("error: [..]/canon/.gitignore is a link, so skiman cannot add `/linked/` to it; replace the link with a file holding the same lines, then run this again\n");

    assert_eq!(read(&real), ".skiman/\n");
    assert_eq!(read(repo.join("skiman.toml")), manifest);
}

/// While `[linked]` tables exist, `check` on the working tree warns when
/// git does not ignore `linked/`, and why when `.gitignore` is a link (git
/// does not read one); the exit status does not change.
#[test]
fn check_warns_while_linked_is_not_ignored() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    std::fs::write(repo.join(".gitignore"), ".skiman/\n").unwrap();
    sandbox.skiman().arg("check").assert().code(0).stderr_eq("");

    append_manifest(&sandbox, COMPANY);
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(0)
        .stderr_eq("warning: git does not ignore `linked/`, so the links in it could be committed; add the line `/linked/` to [..]/canon/.gitignore\n");

    let real = sandbox.root().join("gitignore");
    std::fs::write(&real, ".skiman/\n").unwrap();
    std::fs::remove_file(repo.join(".gitignore")).unwrap();
    std::os::unix::fs::symlink(&real, repo.join(".gitignore")).unwrap();
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(0)
        .stderr_eq("warning: git does not ignore `linked/`, so the links in it could be committed; [..]/canon/.gitignore is a link, and git does not read a `.gitignore` that is a link: replace it with a file that holds the line `/linked/`\n");

    std::fs::remove_file(repo.join(".gitignore")).unwrap();
    std::fs::write(repo.join(".gitignore"), ".skiman/\n/linked/\n").unwrap();
    sandbox.skiman().arg("check").assert().code(0).stderr_eq("");
}

/// When git cannot tell whether `linked/` is ignored (`linked` is a link,
/// so the path lies beyond it), `check` warns with git's own message; the
/// exit status does not change.
#[test]
fn check_warns_when_git_cannot_tell_whether_linked_is_ignored() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    std::fs::write(repo.join(".gitignore"), ".skiman/\n").unwrap();
    append_manifest(&sandbox, COMPANY);
    let elsewhere = sandbox.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, repo.join("linked")).unwrap();

    let output = sandbox.skiman().arg("check").output().unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.starts_with("warning: git cannot tell whether `linked/` is ignored (")
            && stderr.contains("beyond a symbolic link"),
        "{stderr}"
    );
}

/// In a git canon where git cannot tell (`linked` is a link), a `/linked/`
/// line that a later `!/linked/` overrides does not count: the line is
/// appended, decided by the last non-empty line.
#[test]
fn when_git_cannot_tell_the_last_line_decides() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    let gitignore = repo.join(".gitignore");
    std::fs::write(&gitignore, ".skiman/\n/linked/\n!/linked/\n").unwrap();
    let elsewhere = sandbox.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, repo.join("linked")).unwrap();
    folder(&sandbox, "work/company", &["alpha"]);

    sandbox
        .skiman()
        .args(["linked", "add"])
        .arg(sandbox.home().join("work/company"))
        .output()
        .unwrap();

    assert_eq!(
        read(&gitignore),
        ".skiman/\n/linked/\n!/linked/\n/linked/\n"
    );
}
