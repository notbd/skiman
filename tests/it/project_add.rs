//! `skiman project add`, the validation of project roots wherever they are used
//! (a hand-edited `skiman.toml` seen through `sync` and `project adopt`), and
//! `skiman check` on `[projects.<id>]` values (project-scope.md
//! "Configuration").

use std::path::{Path, PathBuf};

use snapbox::cmd::Command;

use crate::support::{Sandbox, Tool, ToolSetup, exists, init};

fn initialised() -> Sandbox {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
}

fn manifest(sandbox: &Sandbox) -> String {
    std::fs::read_to_string(sandbox.skill_repo().join("skiman.toml")).unwrap()
}

/// Appends `text` to `skiman.toml`, as a user editing it by hand.
fn append_manifest(sandbox: &Sandbox, text: &str) {
    let mut toml = manifest(sandbox);
    toml.push_str(text);
    std::fs::write(sandbox.skill_repo().join("skiman.toml"), toml).unwrap();
}

/// Creates the folder `path` and returns it.
fn folder(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    std::fs::create_dir_all(path).unwrap();
    path.to_path_buf()
}

/// Creates the folder `rel` under `HOME` holding an `AGENTS.md`, and returns it.
fn project(sandbox: &Sandbox, rel: &str) -> PathBuf {
    let dir = folder(sandbox.home().join(rel));
    std::fs::write(dir.join("AGENTS.md"), format!("# {rel}\n")).unwrap();
    dir
}

/// `skiman project add <path>`.
fn add_cmd(sandbox: &Sandbox, path: impl AsRef<Path>) -> Command {
    sandbox.skiman().args(["project", "add"]).arg(path.as_ref())
}

fn add(sandbox: &Sandbox, path: impl AsRef<Path>) {
    add_cmd(sandbox, path).assert().code(0);
}

/// Runs `cmd`, expects a refusal (exit 1) whose stderr holds `message`, and
/// proves `skiman.toml` was not written.
#[track_caller]
fn assert_refused(sandbox: &Sandbox, cmd: Command, message: &str) {
    let before = manifest(sandbox);
    let assert = cmd.assert().code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains(message),
        "expected {message:?} in: {stderr}"
    );
    assert_eq!(manifest(sandbox), before, "skiman.toml changed");
}

/// stderr of a command that exits 0.
fn stderr_of(cmd: Command) -> String {
    let assert = cmd.assert().code(0);
    String::from_utf8_lossy(&assert.get_output().stderr).into_owned()
}

/// stdout of `skiman check`, which must fail with exit 1.
fn check_violations(sandbox: &Sandbox) -> String {
    let assert = sandbox.skiman().arg("check").assert().code(1);
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

// ------------------------------------------------------------ add: success

#[test]
fn add_writes_a_tilde_path_and_an_id_from_the_folder_name() {
    let sandbox = initialised();
    let dir = folder(sandbox.home().join("Work/Acme-Corp"));

    // A relative path is taken from the working directory (HOME here).
    let assert = sandbox
        .skiman()
        .args(["project", "add", "Work/Acme-Corp"])
        .assert()
        .code(0);

    assert!(
        manifest(&sandbox).contains("[projects.acme-corp]\npath = \"~/Work/Acme-Corp\"\n"),
        "{}",
        manifest(&sandbox)
    );
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains(&format!(
            "registered project root acme-corp: {}",
            dir.display()
        )),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "{}/projects/acme-corp/",
            sandbox.skill_repo().display()
        )) && stdout.contains("pushed wherever the skill repo is pushed"),
        "add says where the project files will be stored: {stdout}"
    );
    // Registering copies nothing yet, and the written table passes `check`.
    assert!(!exists(sandbox.skill_repo().join("projects")));
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn add_turns_any_folder_name_into_a_valid_id() {
    let sandbox = initialised();
    add(&sandbox, folder(sandbox.home().join("My Projects_2")));
    assert!(
        manifest(&sandbox).contains("[projects.my-projects-2]\npath = \"~/My Projects_2\"\n"),
        "{}",
        manifest(&sandbox)
    );
}

#[test]
fn add_derives_a_valid_id_from_a_long_folder_name() {
    let sandbox = initialised();
    // 63 letters, a space, then more letters: the id is cut at 64 characters.
    let name = format!("{} tail", "a".repeat(63));
    add(&sandbox, folder(sandbox.home().join(&name)));
    assert!(
        manifest(&sandbox).contains(&format!("[projects.{}]\n", "a".repeat(63))),
        "{}",
        manifest(&sandbox)
    );
}

#[test]
fn add_with_id_uses_the_given_id() {
    let sandbox = initialised();
    let dir = folder(sandbox.home().join("Work/Acme-Corp"));
    sandbox
        .skiman()
        .args(["project", "add", "--id", "work"])
        .arg(&dir)
        .assert()
        .code(0);
    let toml = manifest(&sandbox);
    assert!(
        toml.contains("[projects.work]\npath = \"~/Work/Acme-Corp\"\n"),
        "{toml}"
    );
    assert!(!toml.contains("acme-corp"), "{toml}");
}

#[test]
fn add_outside_home_writes_the_absolute_path() {
    let sandbox = initialised();
    let dir = folder(sandbox.root().join("srv/team"));
    add(&sandbox, &dir);
    assert!(
        manifest(&sandbox).contains(&format!("[projects.team]\npath = \"{}\"\n", dir.display())),
        "{}",
        manifest(&sandbox)
    );
}

#[test]
fn add_under_a_linked_home_still_writes_a_tilde_path() {
    let sandbox = initialised();
    let linked_home = sandbox.root().join("home-link");
    std::os::unix::fs::symlink(sandbox.home(), &linked_home).unwrap();
    let dir = folder(sandbox.home().join("dev/app"));
    // The folder is given by its realpath while HOME is the link.
    sandbox
        .skiman()
        .env("HOME", &linked_home)
        .args(["project", "add"])
        .arg(&dir)
        .assert()
        .code(0);
    assert!(
        manifest(&sandbox).contains("[projects.app]\npath = \"~/dev/app\"\n"),
        "{}",
        manifest(&sandbox)
    );
}

#[test]
fn add_rewrites_a_projects_table_written_inline_as_a_standard_table_keeping_its_roots() {
    let sandbox = initialised();
    folder(sandbox.home().join("a"));
    let b = folder(sandbox.home().join("b"));
    let tools = manifest(&sandbox);
    std::fs::write(
        sandbox.skill_repo().join("skiman.toml"),
        format!("projects = {{ a = {{ path = \"~/a\" }} }}\n{tools}"),
    )
    .unwrap();

    add(&sandbox, &b);

    assert_eq!(
        manifest(&sandbox),
        format!("{tools}\n[projects]\na = {{ path = \"~/a\" }}\n\n[projects.b]\npath = \"~/b\"\n")
    );
    sandbox.skiman().arg("check").assert().code(0);
}

// ------------------------------------------------------------ add: refusals

#[test]
fn add_refuses_home() {
    let sandbox = initialised();
    assert_refused(
        &sandbox,
        add_cmd(&sandbox, sandbox.home()),
        "is your home folder or one of its parents",
    );
}

#[test]
fn add_refuses_an_ancestor_of_home() {
    let sandbox = initialised();
    for dir in [sandbox.root(), Path::new("/")] {
        assert_refused(
            &sandbox,
            add_cmd(&sandbox, dir),
            "is your home folder or one of its parents",
        );
    }
}

#[test]
fn add_refuses_the_skill_repo_and_folders_inside_it() {
    let sandbox = initialised();
    let repo = sandbox.skill_repo();
    for dir in [repo.clone(), repo.join("skills")] {
        assert_refused(&sandbox, add_cmd(&sandbox, dir), "is inside the skill repo");
    }
}

#[test]
fn add_refuses_a_tool_presence_dir_and_folders_inside_it() {
    let sandbox = initialised();
    for dir in [
        sandbox.presence_dir(Tool::ClaudeCode),
        sandbox.tool_dir(Tool::ClaudeCode),
    ] {
        assert_refused(
            &sandbox,
            add_cmd(&sandbox, dir),
            "which holds claude-code's global setup",
        );
    }
}

#[test]
fn add_refuses_a_folder_containing_a_linked_presence_dir() {
    // `~/.codex` links to a dotfiles folder outside HOME: that folder holds
    // codex's global setup, compared by realpath.
    let sandbox = Sandbox::builder()
        .tool(Tool::Codex, ToolSetup::Absent)
        .build();
    init(&sandbox);
    let dotfiles = folder(sandbox.root().join("dotfiles"));
    folder(dotfiles.join("codex"));
    std::os::unix::fs::symlink(dotfiles.join("codex"), sandbox.presence_dir(Tool::Codex)).unwrap();
    assert_refused(
        &sandbox,
        add_cmd(&sandbox, dotfiles),
        "which holds codex's global setup",
    );
}

#[test]
fn add_refuses_a_folder_containing_another_root() {
    let sandbox = initialised();
    add(&sandbox, folder(sandbox.home().join("Work/acme-us")));
    assert_refused(
        &sandbox,
        add_cmd(&sandbox, sandbox.home().join("Work")),
        "overlaps project root `acme-us`",
    );
}

#[test]
fn add_refuses_a_folder_inside_another_root() {
    let sandbox = initialised();
    let work = folder(sandbox.home().join("Work"));
    add(&sandbox, &work);
    assert_refused(
        &sandbox,
        add_cmd(&sandbox, folder(work.join("acme-us"))),
        "overlaps project root `work`",
    );
}

#[test]
fn add_compares_other_roots_by_realpath() {
    // A hand-written root reaches `~/Work` through a symlink.
    let sandbox = initialised();
    let work = folder(sandbox.home().join("Work"));
    std::os::unix::fs::symlink(&work, sandbox.home().join("w")).unwrap();
    append_manifest(&sandbox, "\n[projects.aliased]\npath = \"~/w\"\n");
    assert_refused(
        &sandbox,
        add_cmd(&sandbox, folder(work.join("acme-us"))),
        "overlaps project root `aliased`",
    );
}

#[test]
fn add_refuses_a_taken_id() {
    let sandbox = initialised();
    add(&sandbox, folder(sandbox.home().join("one/app")));
    // Same folder name, so the same default id.
    assert_refused(
        &sandbox,
        add_cmd(&sandbox, folder(sandbox.home().join("two/app"))),
        "skiman.toml already has [projects.app]; pick another id with --id",
    );
}

#[test]
fn add_refuses_an_invalid_id() {
    let sandbox = initialised();
    let dir = folder(sandbox.home().join("app"));
    for id in ["Work", "my_app", "-app", "a--b"] {
        assert_refused(
            &sandbox,
            add_cmd(&sandbox, &dir).arg(format!("--id={id}")),
            &format!("`{id}` is not a valid project root id"),
        );
    }
}

#[test]
fn add_refuses_a_folder_name_with_no_usable_id_and_asks_for_one() {
    let sandbox = initialised();
    assert_refused(
        &sandbox,
        add_cmd(&sandbox, folder(sandbox.home().join("新建"))),
        "give one with --id",
    );
}

#[test]
fn add_refuses_a_missing_folder_or_a_file() {
    let sandbox = initialised();
    let file = sandbox.home().join("notes.md");
    std::fs::write(&file, "x\n").unwrap();
    for path in [sandbox.home().join("missing"), file] {
        assert_refused(
            &sandbox,
            add_cmd(&sandbox, path),
            "is not an existing folder",
        );
    }
}

// ------------------------------------------ hand-edited roots: sync, adopt

#[test]
fn sync_skips_home_and_its_ancestors_with_one_warning_each() {
    let sandbox = initialised();
    append_manifest(
        &sandbox,
        "\n[projects.home]\npath = \"~\"\n\n[projects.top]\npath = \"/\"\n",
    );
    let stderr = stderr_of(sandbox.skiman().arg("sync"));
    for id in ["home", "top"] {
        let warning = format!("warning: project root `{id}` is skipped: ");
        assert_eq!(stderr.matches(&warning).count(), 1, "{id}: {stderr}");
    }
    assert!(
        stderr.contains("is your home folder or one of its parents; register project folders instead; fix [projects.home] in skiman.toml"),
        "{stderr}"
    );
    assert!(!exists(sandbox.skill_repo().join("projects")));
}

#[test]
fn sync_skips_both_overlapping_roots() {
    // `b` reaches the parent of `a` through a symlink: overlap is by realpath.
    let sandbox = initialised();
    let work = project(&sandbox, "w");
    project(&sandbox, "w/a");
    std::os::unix::fs::symlink(&work, sandbox.home().join("alias")).unwrap();
    append_manifest(
        &sandbox,
        "\n[projects.a]\npath = \"~/w/a\"\n\n[projects.b]\npath = \"~/alias\"\n",
    );
    let stderr = stderr_of(sandbox.skiman().arg("sync"));
    for (id, other) in [("a", "b"), ("b", "a")] {
        let warning = format!(
            "warning: project root `{id}` is skipped: it overlaps project root `{other}`; roots must not contain each other"
        );
        assert_eq!(stderr.matches(&warning).count(), 1, "{id}: {stderr}");
    }
}

#[test]
fn adopt_skips_invalid_roots_and_adopts_under_the_valid_one() {
    let sandbox = initialised();
    project(&sandbox, "w");
    project(&sandbox, "w/a");
    let ok = project(&sandbox, "ok");
    append_manifest(
        &sandbox,
        "\n[projects.home]\npath = \"~\"\n\n[projects.a]\npath = \"~/w/a\"\n\n[projects.b]\npath = \"~/w\"\n\n[projects.ok]\npath = \"~/ok\"\n",
    );

    let stderr = stderr_of(sandbox.skiman().args(["project", "adopt", "--yes"]));

    for id in ["home", "a", "b"] {
        assert!(
            stderr.contains(&format!("warning: project root `{id}` is skipped: ")),
            "{id}: {stderr}"
        );
        assert!(
            !exists(sandbox.skill_repo().join("projects").join(id)),
            "{id}"
        );
    }
    assert_eq!(
        std::fs::read(sandbox.skill_repo().join("projects/ok/AGENTS.md")).unwrap(),
        std::fs::read(ok.join("AGENTS.md")).unwrap()
    );
}

#[test]
fn adopt_refuses_a_path_inside_a_skipped_root() {
    let sandbox = initialised();
    let work = project(&sandbox, "w");
    project(&sandbox, "w/a");
    append_manifest(
        &sandbox,
        "\n[projects.a]\npath = \"~/w/a\"\n\n[projects.b]\npath = \"~/w\"\n",
    );
    let assert = sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .arg(work.join("AGENTS.md"))
        .assert()
        .code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("is not inside a project root"), "{stderr}");
    assert!(!exists(sandbox.skill_repo().join("projects")));
}

// ------------------------------------------------- check on [projects]

#[test]
fn check_reports_an_unknown_key_in_a_project_table() {
    let sandbox = initialised();
    append_manifest(
        &sandbox,
        "\n[projects.work]\npath = \"~/Work\"\nexcludes = [\"_archive\"]\n",
    );
    let out = check_violations(&sandbox);
    assert!(
        out.contains("unknown-key: unknown key `excludes` in [projects.work] (allowed: path, exclude, skill_dirs)"),
        "{out}"
    );
}

#[test]
fn check_reports_a_path_that_is_neither_absolute_nor_under_home() {
    let sandbox = initialised();
    let cases = [
        ("relative", "Work"),
        ("bare-tilde", "~"),
        ("other-user", "~other/Work"),
    ];
    for (id, path) in cases {
        append_manifest(&sandbox, &format!("\n[projects.{id}]\npath = \"{path}\"\n"));
    }
    let out = check_violations(&sandbox);
    for (id, path) in cases {
        assert!(
            out.contains(&format!(
                "toml-value: `path` in [projects.{id}] must be a string: an absolute path or one starting with `~/`"
            )),
            "{path}: {out}"
        );
    }
}

/// `skiman check` output for one root whose `skill_dirs` is `dirs`.
fn check_skill_dirs(dirs: &str) -> String {
    let sandbox = initialised();
    append_manifest(
        &sandbox,
        &format!("\n[projects.work]\npath = \"~/Work\"\nskill_dirs = {dirs}\n"),
    );
    check_violations(&sandbox)
}

#[test]
fn check_reports_an_absolute_skill_dirs_entry() {
    let out = check_skill_dirs(r#"["/srv/skills", "~/skills"]"#);
    for dir in ["/srv/skills", "~/skills"] {
        assert!(
            out.contains(&format!(
                "`skill_dirs` entry \"{dir}\" in [projects.work] must be relative to the root"
            )),
            "{dir}: {out}"
        );
    }
}

#[test]
fn check_reports_a_skill_dirs_entry_with_dot_dot() {
    let out = check_skill_dirs(r#"["../shared", "a/../b"]"#);
    for dir in ["../shared", "a/../b"] {
        assert!(
            out.contains(&format!(
                "`skill_dirs` entry \"{dir}\" in [projects.work] must be a plain relative path"
            )),
            "{dir}: {out}"
        );
    }
}

#[test]
fn check_reports_a_skill_dirs_entry_holding_a_tool_skill_dir() {
    let out =
        check_skill_dirs(r#"[".claude/skills", "team/.claude/skills", "x/.agents/skills/y"]"#);
    for dir in [
        ".claude/skills",
        "team/.claude/skills",
        "x/.agents/skills/y",
    ] {
        assert!(
            out.contains(&format!(
                "`skill_dirs` entry \"{dir}\" in [projects.work] holds a tool's own project skill folder"
            )),
            "{dir}: {out}"
        );
    }
}

#[test]
fn check_reports_a_skill_dirs_entry_nested_in_another() {
    // Listed before its parent, so the order does not matter.
    let out = check_skill_dirs(r#"["shared/team", "shared"]"#);
    assert!(
        out.contains(
            "`skill_dirs` entry \"shared/team\" in [projects.work] lies inside another `skill_dirs` entry"
        ),
        "{out}"
    );
    assert!(!out.contains("entry \"shared\" "), "{out}");
}

#[test]
fn check_accepts_valid_project_tables() {
    // `shared` and `shared2` share a prefix but neither lies inside the other.
    let sandbox = initialised();
    append_manifest(
        &sandbox,
        "\n[projects.work]\npath = \"~/Work/Acme-Corp\"\nexclude = [\"_archive\", \"a/b\"]\nskill_dirs = [\"acme-core/skills\", \"shared\", \"shared2\"]\n\n[projects.srv]\npath = \"/srv/team\"\n",
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn check_refuses_exclude_entries_that_can_match_nothing() {
    let sandbox = initialised();
    let dir = folder(sandbox.home().join("work"));
    add(&sandbox, &dir);
    append_manifest(
        &sandbox,
        "exclude = [\"./_archive\", \"/abs/backup\", \"_archive\", \"a/b\"]\n",
    );
    let out = check_violations(&sandbox);
    assert!(out.contains("`exclude` entry \"./_archive\""), "{out}");
    assert!(out.contains("`exclude` entry \"/abs/backup\""), "{out}");
    assert!(!out.contains("\"_archive\" in"), "{out}");
    assert!(!out.contains("\"a/b\" in"), "{out}");
}

#[test]
fn add_explains_a_folder_name_without_letters_or_digits() {
    let sandbox = initialised();
    let dir = folder(sandbox.home().join("新建"));
    assert_refused(
        &sandbox,
        add_cmd(&sandbox, &dir),
        "has no letters or digits to make a root id from; give one with --id",
    );
}

#[test]
fn add_refuses_a_folder_that_holds_a_root_missing_on_this_machine() {
    let sandbox = initialised();
    append_manifest(&sandbox, "\n[projects.sub]\npath = \"~/x/sub\"\n");
    let dir = folder(sandbox.home().join("x"));
    assert_refused(
        &sandbox,
        add_cmd(&sandbox, &dir).args(["--id", "x"]),
        "it overlaps project root `sub`",
    );
}

#[test]
fn add_refuses_a_folder_that_holds_a_missing_root_also_under_a_linked_home() {
    let sandbox = initialised();
    // HOME seen through a link: the folder is given by realpath.
    let linked_home = sandbox.root().join("linked-home");
    std::os::unix::fs::symlink(sandbox.home(), &linked_home).unwrap();
    append_manifest(&sandbox, "\n[projects.sub]\npath = \"~/x/sub\"\n");
    let dir = folder(sandbox.home().join("x"));
    assert_refused(
        &sandbox,
        add_cmd(&sandbox, &dir)
            .args(["--id", "x"])
            .env("HOME", &linked_home),
        "it overlaps project root `sub`",
    );
}
