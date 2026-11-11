//! `skiman doctor`: every problem of the setup in one place, and the
//! read-only guarantee of `list`, `diff` and `doctor`.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use support::{Sandbox, Tool, ToolSetup, tree};

const BINARY: &str = env!("CARGO_BIN_EXE_skiman");

fn init(sandbox: &Sandbox, session_hooks: &str) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", session_hooks])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A folder holding a `SKILL.md`, as another installer would leave it.
fn skill_folder(dir: impl AsRef<Path>, name: &str) {
    write(
        dir.as_ref().join("SKILL.md"),
        &format!("---\nname: {name}\ndescription: d\n---\n"),
    );
}

/// Installs `acme/<name>`, a one-skill upstream, with `skiman add`.
fn add(sandbox: &Sandbox, name: &str) {
    let up = sandbox.upstream(&format!("acme/{name}"));
    up.write(
        "SKILL.md",
        format!("---\nname: {name}\ndescription: d\n---\n"),
    );
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", &format!("acme/{name}"), "--yes"])
        .assert()
        .code(0);
}

/// Runs `skiman doctor`, asserts exit 0 and empty stderr, returns stdout.
fn doctor(sandbox: &Sandbox) -> String {
    let output = sandbox
        .skiman()
        .arg("doctor")
        .assert()
        .code(0)
        .stderr_eq("");
    String::from_utf8(output.get_output().stdout.clone()).unwrap()
}

fn assert_line(stdout: &str, line: &str) {
    assert!(
        stdout.lines().any(|l| l == line),
        "missing line:\n{line}\nin doctor output:\n{stdout}"
    );
}

#[test]
fn doctor_reports_a_healthy_setup() {
    let sandbox = Sandbox::new();
    init(&sandbox, "claude-code");
    let repo = sandbox.skill_repo();
    let home = sandbox.home();
    let home = home.display();

    sandbox
        .skiman()
        .arg("doctor")
        .assert()
        .code(0)
        .stdout_eq(format!(
            "\
skill repo: {repo}
recorded binary: {BINARY}
git: version [..]
git hooks: installed in {repo}/.git/hooks
session-start hook for claude-code: installed in {home}/.claude/settings.json
session-start hook for codex: not added: Codex runs only the hooks you have trusted with `/hooks`; after a change, run `skiman sync`, or rely on the skill repo's git hooks
claude-code: installed; tool directory {home}/.claude/skills; instruction file {home}/.claude/CLAUDE.md
codex: installed; tool directory {home}/.agents/skills; instruction file {home}/.codex/AGENTS.md
antigravity: installed; tool directory {home}/.gemini/config/skills; instruction file {home}/.gemini/GEMINI.md
",
            repo = repo.display()
        ))
        .stderr_eq("");
}

#[test]
fn doctor_lists_every_foreign_entry_on_every_run() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let foreign = sandbox.tool_dir(Tool::ClaudeCode).join("pdf");
    skill_folder(&foreign, "pdf");
    let line = format!(
        "foreign skill {} is not managed by skiman; bring it in with `skiman adopt pdf --from claude-code`",
        foreign.display()
    );
    // sync reports it once; after that the hook stays quiet.
    sandbox.skiman().arg("sync").assert().code(0);
    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("");

    assert_line(&doctor(&sandbox), &line);
    assert_line(&doctor(&sandbox), &line);
}

#[test]
fn doctor_reports_each_problem_in_one_place() {
    let sandbox = Sandbox::builder()
        .tool(Tool::Antigravity, ToolSetup::Absent)
        .build();
    init(&sandbox, "none");
    let repo = sandbox.skill_repo();
    let home = sandbox.home();
    add(&sandbox, "pdf");
    add(&sandbox, "gone");
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes"])
        .assert()
        .code(0);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    let codex = sandbox.tool_dir(Tool::Codex);

    // Antigravity is enabled but not present on this machine.
    let manifest = repo.join("skiman.toml");
    let text = read(&manifest).replace(
        "enabled = [\"claude-code\", \"codex\"]",
        "enabled = [\"claude-code\", \"codex\", \"antigravity\"]",
    );
    write(&manifest, &text);
    // The recorded binary is gone.
    let config = sandbox.config_home().join("skiman/config.toml");
    let gone_binary = sandbox.root().join("old/skiman");
    write(
        &config,
        &read(&config).replace(BINARY, gone_binary.to_str().unwrap()),
    );
    // A git hook was deleted.
    std::fs::remove_file(repo.join(".git/hooks/post-merge")).unwrap();
    // Occupied: a regular file where skill notes goes.
    std::fs::remove_file(claude.join("notes")).unwrap();
    write(claude.join("notes"), "not a skill");
    // A dangling link skiman does not own.
    std::os::unix::fs::symlink(home.join("nowhere"), claude.join("old")).unwrap();
    // Managed elsewhere.
    skill_folder(claude.join("synced/web"), "web");
    // A foreign skill in the Codex tool directory.
    skill_folder(codex.join("csv"), "csv");
    // An instruction file skiman does not own, and Codex's override file.
    std::fs::remove_file(sandbox.instruction_file(Tool::ClaudeCode)).unwrap();
    write(sandbox.instruction_file(Tool::ClaudeCode), "# mine\n");
    let override_file = sandbox.presence_dir(Tool::Codex).join("AGENTS.override.md");
    write(&override_file, "# override\n");
    // Unresolved conflicts in pdf.
    write(
        repo.join("skills/pdf/SKILL.md"),
        "---\nname: pdf\ndescription: d\n---\n<<<<<<< local (skiman)\na\n=======\nb\n>>>>>>> upstream (skiman)\n",
    );
    write(repo.join("skills/pdf/run.sh.skiman-upstream"), "b\n");
    // Leftover records: the folder of gone was deleted by hand.
    std::fs::remove_dir_all(repo.join("skills/gone")).unwrap();
    // A tmp leftover.
    write(
        repo.join(".skiman/tmp/20261104T120000Z/candidate/SKILL.md"),
        "x",
    );

    let out = doctor(&sandbox);
    let r = repo.display();
    for line in [
        format!("skill repo: {r}"),
        format!(
            "recorded binary: {} no longer exists; the git hooks and session-start hooks call it, so run `skiman init {r}` again with the skiman you use now",
            gone_binary.display()
        ),
        format!("git hook {r}/.git/hooks/post-merge: missing; run `skiman init {r}` again"),
        format!(
            "git hook {r}/.git/hooks/pre-commit: skiman's, but not what `skiman init` writes now; run `skiman init {r}` again"
        ),
        format!(
            "session-start hook for claude-code: not installed in {}; add it with `skiman init {r} --session-hooks claude-code`",
            home.join(".claude/settings.json").display()
        ),
        format!(
            "antigravity: enabled, but {} does not exist, so skiman skips it",
            home.join(".gemini").display()
        ),
        format!(
            "{} is occupied by an entry skiman does not own, so skill notes is not linked there; move it away, then run `skiman sync`",
            claude.join("notes").display()
        ),
        format!(
            "dangling link {} -> {}: skiman does not own it and leaves it alone; delete it if nothing needs it",
            claude.join("old").display(),
            home.join("nowhere").display()
        ),
        format!(
            "managed elsewhere: {}; skiman never touches it",
            claude.join("synced").display()
        ),
        format!(
            "foreign skill {} is not managed by skiman; bring it in with `skiman adopt csv --from codex`",
            codex.join("csv").display()
        ),
        format!(
            "instruction file {} is not owned by skiman, so the shared body is not linked there; move it away, then run `skiman sync`",
            sandbox.instruction_file(Tool::ClaudeCode).display()
        ),
        format!(
            "{} exists, so codex loads it instead of {}; move it away to use the shared body",
            override_file.display(),
            sandbox.instruction_file(Tool::Codex).display()
        ),
        "skills/pdf/SKILL.md:5: unresolved-conflict: conflict marker from an update; resolve the conflict and remove the markers".to_owned(),
        "skills/pdf/SKILL.md:9: unresolved-conflict: conflict marker from an update; resolve the conflict and remove the markers".to_owned(),
        "skills/pdf/run.sh.skiman-upstream: unresolved-conflict: conflict record from an update; resolve the conflict, then delete it".to_owned(),
        "skiman.lock:[..]".to_owned(),
        format!(
            "tmp leftover {r}/.skiman/tmp/20261104T120000Z; the next write command moves it to a backup"
        ),
    ] {
        if let Some(prefix) = line.strip_suffix("[..]") {
            assert!(
                out.lines().any(|l| l.starts_with(prefix) && l.contains("leftover lock entry for gone")),
                "missing leftover lock entry line in:\n{out}"
            );
        } else {
            assert_line(&out, &line);
        }
    }
    assert!(
        out.lines().any(|l| l.starts_with("skiman.toml:")
            && l.contains("leftover-record: leftover [skills.gone.source] table")),
        "missing leftover source table line in:\n{out}"
    );
    assert_line(
        &out,
        "upstream/gone: leftover-record: leftover upstream snapshot, but skills/gone does not exist; run `skiman remove gone --yes` (after a hand rename, rename this too)",
    );
}

#[test]
fn doctor_reports_aliased_tool_directories_and_instruction_locations() {
    let sandbox = Sandbox::new();
    let gemini_skills = sandbox.tool_dir(Tool::Antigravity);
    std::fs::remove_dir_all(&gemini_skills).ok();
    std::fs::create_dir_all(gemini_skills.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(sandbox.tool_dir(Tool::Codex), &gemini_skills).unwrap();
    init(&sandbox, "none");
    let home = sandbox.home();

    assert_line(
        &doctor(&sandbox),
        &format!(
            "antigravity: installed; tool directory {} (same directory as codex's); instruction file {}",
            gemini_skills.display(),
            home.join(".gemini/GEMINI.md").display()
        ),
    );
}

/// A `git` that only prints a version, first on a `PATH` without the real one.
fn fake_git(sandbox: &Sandbox, version_line: Option<&str>) -> String {
    let bin = sandbox.root().join("fake-bin");
    std::fs::create_dir_all(&bin).unwrap();
    if let Some(line) = version_line {
        let git = bin.join("git");
        write(&git, &format!("#!/bin/sh\necho '{line}'\n"));
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    format!("{}:/bin", bin.display())
}

#[test]
fn doctor_reports_a_missing_or_too_old_git() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");

    let path = fake_git(&sandbox, None);
    let output = sandbox
        .skiman()
        .env("PATH", path)
        .arg("doctor")
        .assert()
        .code(0);
    let out = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert_line(&out, "git: not found; skiman requires git 2.43 or newer");

    let path = fake_git(&sandbox, Some("git version 2.39.5 (Apple Git-154)"));
    let output = sandbox
        .skiman()
        .env("PATH", path)
        .arg("doctor")
        .assert()
        .code(0);
    let out = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert_line(
        &out,
        "git: version 2.39.5 is too old; skiman requires 2.43 or newer",
    );
}

#[test]
fn doctor_shows_tmp_content_as_in_use_while_the_repo_lock_is_held() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let repo = sandbox.skill_repo();
    let staged = repo.join(".skiman/tmp/20261104T120000Z");
    write(staged.join("candidate/SKILL.md"), "x");

    let lock = std::fs::File::options()
        .write(true)
        .open(repo.join(".skiman/lock"))
        .unwrap();
    lock.lock().unwrap();
    let out = doctor(&sandbox);
    assert_line(
        &out,
        &format!(
            "{} is in use: another skiman command holds the repo lock",
            staged.display()
        ),
    );
    assert!(!out.contains("tmp leftover"), "{out}");
    drop(lock);

    assert_line(
        &doctor(&sandbox),
        &format!(
            "tmp leftover {}; the next write command moves it to a backup",
            staged.display()
        ),
    );
}

/// Clarification 7 on #1: the lock entry was written, then moving the old
/// folders to the backup failed, so they stay in `.skiman/tmp/`. `update`
/// is another ticket: the state it leaves is built by hand here.
#[test]
fn doctor_lists_old_folders_an_update_left_in_tmp() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    add(&sandbox, "pdf");
    let repo = sandbox.skill_repo();
    let staged = repo.join(".skiman/tmp/20261104T120000Z");
    for (from, to) in [
        ("skills/pdf", "old/skills/pdf"),
        ("upstream/pdf", "old/upstream/pdf"),
    ] {
        let dest = staged.join(to);
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::copy(repo.join(from).join("SKILL.md"), dest.join("SKILL.md")).unwrap();
    }

    assert_line(
        &doctor(&sandbox),
        &format!(
            "tmp leftover {}; the next write command moves it to a backup",
            staged.display()
        ),
    );
}

/// Every file under the sandbox root: skill repo (with `.git` and
/// `.skiman/tmp/`), tool directories, instruction files, config and state.
fn world(sandbox: &Sandbox) -> std::collections::BTreeMap<PathBuf, String> {
    tree(sandbox.root())
}

#[test]
fn list_diff_and_doctor_change_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox, "claude-code");
    add(&sandbox, "pdf");
    let repo = sandbox.skill_repo();
    write(repo.join("skills/pdf/notes.md"), "local\n");
    skill_folder(sandbox.tool_dir(Tool::Codex).join("csv"), "csv");
    write(
        repo.join(".skiman/tmp/20261104T120000Z/candidate/SKILL.md"),
        "x",
    );
    let before = world(&sandbox);

    for args in [&["list"][..], &["diff", "pdf"], &["doctor"]] {
        sandbox.skiman_confined().args(args).assert().code(0);
        assert_eq!(world(&sandbox), before, "skiman {args:?} changed a file");
    }
}
