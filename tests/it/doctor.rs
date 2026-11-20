//! `skiman doctor`: every problem of the setup in one place, and the
//! read-only guarantee of `list`, `diff` and `doctor`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::support::{Sandbox, Tool, ToolSetup, read, tree, write};

const BINARY: &str = env!("CARGO_BIN_EXE_skiman");

fn init(sandbox: &Sandbox, session_hooks: &str) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", session_hooks])
        .arg(sandbox.canon())
        .assert()
        .code(0);
}

/// A folder holding a `SKILL.md`, as another installer would leave it.
fn skill_folder(dir: impl AsRef<Path>, name: &str) {
    write(
        dir.as_ref().join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d\n---\n"),
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

/// Runs `skiman doctor`, asserts exit `exit` and empty stderr, returns stdout.
fn doctor(sandbox: &Sandbox, exit: i32) -> String {
    let output = sandbox
        .skiman()
        .arg("doctor")
        .assert()
        .code(exit)
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
    init(&sandbox, "claude-code,antigravity");
    let repo = sandbox.canon();
    let home = sandbox.home();
    let home = home.display();

    sandbox
        .skiman()
        .arg("doctor")
        .assert()
        .code(0)
        .stdout_eq(format!(
            "\
canon: {repo}
recorded binary: {BINARY}
git: version [..]
git hooks: installed in {repo}/.git/hooks
session-start hook for claude-code: installed in {home}/.claude/settings.json
session-start hook for codex: not added: Codex runs only the hooks you have trusted with `/hooks`; run `skiman sync` before you start a Codex session, or rely on the canon's git hooks
session-start hook for antigravity: installed in {home}/.gemini/config/hooks.json
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

    assert_line(&doctor(&sandbox, 0), &line);
    assert_line(&doctor(&sandbox, 0), &line);
}

#[test]
fn doctor_reports_each_problem_in_one_place() {
    let sandbox = Sandbox::builder()
        .tool(Tool::Antigravity, ToolSetup::Absent)
        .build();
    init(&sandbox, "none");
    let repo = sandbox.canon();
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
        read(&config).replace(BINARY, gone_binary.to_str().unwrap()),
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

    let out = doctor(&sandbox, 3);
    let r = repo.display();
    for line in [
        format!("canon: {r}"),
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
        &doctor(&sandbox, 0),
        &format!(
            "antigravity: installed; tool directory {} (same directory as codex's); instruction file {}",
            gemini_skills.display(),
            home.join(".gemini/GEMINI.md").display()
        ),
    );
}

/// A `git` that only prints a version, alone on the `PATH` (the real one is
/// in `/bin` on a system that merged it with `/usr/bin`).
fn fake_git(sandbox: &Sandbox, version_line: Option<&str>) -> String {
    let bin = sandbox.root().join("fake-bin");
    std::fs::create_dir_all(&bin).unwrap();
    if let Some(line) = version_line {
        let git = bin.join("git");
        write(&git, format!("#!/bin/sh\necho '{line}'\n"));
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin.display().to_string()
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
        .code(3);
    let out = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert_line(&out, "git: not found; skiman requires git 2.43 or newer");

    let path = fake_git(&sandbox, Some("git version 2.39.5 (Apple Git-154)"));
    let output = sandbox
        .skiman()
        .env("PATH", path)
        .arg("doctor")
        .assert()
        .code(3);
    let out = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert_line(
        &out,
        "git: version 2.39.5 is too old; skiman requires 2.43 or newer",
    );
}

#[test]
fn doctor_shows_tmp_content_as_in_use_while_the_canon_lock_is_held() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let repo = sandbox.canon();
    let staged = repo.join(".skiman/tmp/20261104T120000Z");
    write(staged.join("candidate/SKILL.md"), "x");

    let lock = std::fs::File::options()
        .write(true)
        .open(repo.join(".skiman/lock"))
        .unwrap();
    lock.lock().unwrap();
    let out = doctor(&sandbox, 0);
    assert_line(
        &out,
        &format!(
            "{} is in use: another skiman command holds the canon lock",
            staged.display()
        ),
    );
    assert!(!out.contains("tmp leftover"), "{out}");
    drop(lock);

    assert_line(
        &doctor(&sandbox, 0),
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
    let repo = sandbox.canon();
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
        &doctor(&sandbox, 0),
        &format!(
            "tmp leftover {}; the next write command moves it to a backup",
            staged.display()
        ),
    );
}

/// Every file under the sandbox root: canon (with `.git` and
/// `.skiman/tmp/`), tool directories, instruction files, config and state.
fn world(sandbox: &Sandbox) -> std::collections::BTreeMap<PathBuf, String> {
    tree(sandbox.root())
}

#[test]
fn list_diff_and_doctor_change_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox, "claude-code");
    add(&sandbox, "pdf");
    let repo = sandbox.canon();
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

#[test]
fn doctor_names_each_instruction_file_antigravity_loads_on_top_of_the_shared_body() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let gemini = sandbox.presence_dir(Tool::Antigravity);
    write(gemini.join("AGENTS.md"), "# extra\n");
    write(gemini.join("config/AGENTS.md"), "# extra\n");

    let out = doctor(&sandbox, 0);
    for rel in ["AGENTS.md", "config/AGENTS.md"] {
        assert_line(
            &out,
            &format!(
                "{} exists, so antigravity also loads it on top of {}",
                gemini.join(rel).display(),
                gemini.join("GEMINI.md").display()
            ),
        );
    }
    assert!(
        !out.contains("config/GEMINI.md"),
        "a file that does not exist is not named:\n{out}"
    );
}

#[test]
fn doctor_reports_antigravitys_session_start_hook_missing_and_different() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let r = sandbox.canon();
    let hooks = sandbox.home().join(".gemini/config/hooks.json");

    assert_line(
        &doctor(&sandbox, 0),
        &format!(
            "session-start hook for antigravity: not installed in {}; add it with `skiman init {} --session-hooks antigravity`",
            hooks.display(),
            r.display()
        ),
    );

    write(
        &hooks,
        r#"{"skiman": {"SessionStart": [{"type": "command", "command": "/old/skiman sync --hook"}]}}"#,
    );
    assert_line(
        &doctor(&sandbox, 3),
        &format!(
            "session-start hook for antigravity: {} holds a skiman hook that differs from what `skiman init` writes now; rewrite it with `skiman init {} --session-hooks antigravity`",
            hooks.display(),
            r.display()
        ),
    );
}

/// Each problem line of doctor's distribution part is a warning the next
/// `skiman sync` prints, decided by sync's own code: a linked skill whose
/// `linked/<id>` is blocked is never offered, so a file under its name in a
/// tool directory is no problem. Both exit 3.
#[test]
fn doctor_reports_what_the_next_sync_reports_and_exits_like_it() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    skill_folder(sandbox.home().join("outside/notes"), "notes");
    let manifest = sandbox.canon().join("skiman.toml");
    let mut text = std::fs::read_to_string(&manifest).unwrap();
    text.push_str("\n[linked.notes]\npath = \"~/outside\"\n");
    write(&manifest, text);
    write(sandbox.canon().join("linked/notes"), "in the way");
    let taken = sandbox.tool_dir(Tool::ClaudeCode).join("notes");
    write(&taken, "in the way");
    skill_folder(sandbox.tool_dir(Tool::Codex).join("theirs"), "theirs");

    let out = doctor(&sandbox, 3);
    let sync = sandbox.skiman().arg("sync").output().unwrap();

    assert_eq!(sync.status.code(), Some(3));
    let warnings: Vec<String> = String::from_utf8(sync.stderr)
        .unwrap()
        .lines()
        .filter_map(|l| l.strip_prefix("warning: ").map(str::to_owned))
        .collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for warning in &warnings {
        assert_line(&out, warning);
    }
    assert!(
        !out.contains(&format!("{} is occupied", taken.display())),
        "{out}"
    );
}

/// doctor names each write the next sync makes; a setup that only needs
/// a sync is healthy (exit 0), and after the sync doctor names none. A
/// skill moved into a group: doctor names each retarget, sync makes it.
#[test]
fn doctor_lists_the_links_the_next_sync_makes() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    skill_folder(sandbox.canon().join("skills/notes"), "notes");

    let out = doctor(&sandbox, 0);
    for tool in Tool::ALL {
        let link = sandbox.tool_dir(tool).join("notes");
        assert_line(
            &out,
            &format!("{}: the next `skiman sync` links it", link.display()),
        );
        assert!(
            std::fs::symlink_metadata(&link).is_err(),
            "doctor wrote {}",
            link.display()
        );
    }
    sandbox.skiman().arg("sync").assert().code(0);
    assert!(!doctor(&sandbox, 0).contains("the next `skiman sync`"));

    let group = sandbox.canon().join("skills/writing");
    std::fs::create_dir(&group).unwrap();
    std::fs::rename(sandbox.canon().join("skills/notes"), group.join("notes")).unwrap();
    let out = assert_doctor_foresees_sync(&sandbox);
    for tool in Tool::ALL {
        let link = sandbox.tool_dir(tool).join("notes");
        let line = format!("{}: the next `skiman sync` retargets it", link.display());
        assert_line(&out, &line);
    }
}

/// A hook doctor tells you to add skiman's line to by hand is not a problem
/// doctor can judge (it cannot tell whether the line is there): exit 0.
#[test]
fn doctor_exits_0_for_a_hook_that_is_not_skimans() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let hook = sandbox.canon().join(".git/hooks/pre-commit");
    write(&hook, "#!/bin/sh\necho mine\n");

    let out = doctor(&sandbox, 0);
    assert!(
        out.contains("not skiman's; add this line to it by hand"),
        "{out}"
    );
}

/// A tool directory doctor cannot read is one error line; the rest of the
/// diagnosis still runs, and doctor exits 4.
#[test]
fn doctor_reports_an_unreadable_tool_directory_and_goes_on() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let leftover = sandbox.canon().join(".skiman/tmp/left");
    std::fs::create_dir_all(&leftover).unwrap();
    let dir = sandbox.tool_dir(Tool::Codex);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();

    let out = assert_doctor_foresees_sync(&sandbox);

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_line(
        &out,
        &format!(
            "cannot read {}: Permission denied (os error 13)",
            dir.display()
        ),
    );
    let left = format!(
        "tmp leftover {}; the next write command moves it to a backup",
        leftover.display()
    );
    assert_line(&out, &left);
}

/// What doctor could change: every file under the sandbox's HOME, config
/// and state folders and the canon, and the canon's git index byte for byte.
fn on_disk(sandbox: &Sandbox) -> (Vec<std::collections::BTreeMap<PathBuf, String>>, Vec<u8>) {
    let dirs = [
        sandbox.home(),
        sandbox.config_home(),
        sandbox.state_home(),
        sandbox.canon(),
    ];
    let index = std::fs::read(sandbox.canon().join(".git/index")).unwrap_or_default();
    (dirs.iter().map(tree).collect(), index)
}

/// Runs doctor, then `skiman sync`: doctor said what sync then did and
/// reported, from what was there before either ran (a line
/// `<path>: the next \`skiman sync\` <verb>s it` for each change sync made,
/// each sync warning and error as a line), both exit the same, and doctor
/// changed nothing. Returns doctor's stdout.
pub(crate) fn assert_doctor_foresees_sync(sandbox: &Sandbox) -> String {
    let before = on_disk(sandbox);
    let doctor = sandbox.skiman().arg("doctor").output().unwrap();
    assert_eq!(on_disk(sandbox), before, "doctor changed something");
    let sync = sandbox.skiman().arg("sync").output().unwrap();
    let out = String::from_utf8(doctor.stdout).unwrap();
    assert_eq!(doctor.status.code(), sync.status.code(), "{out}\n{sync:?}");
    let synced = String::from_utf8(sync.stdout.clone()).unwrap();
    // And the other way: each link or mirror write doctor names, sync made.
    for line in out.lines() {
        let Some((path, verb)) = line.split_once(": the next `skiman sync` ") else {
            continue;
        };
        let done = match verb {
            "links it" => "linked",
            "retargets it" => "retargeted",
            "removes it" => "removed",
            "mirrors it" => "mirrored",
            other => panic!("doctor printed {other}"),
        };
        assert_line(&synced, &format!("{done} {path}"));
    }
    for line in String::from_utf8(sync.stdout).unwrap().lines() {
        let (verb, path) = line.split_once(' ').unwrap();
        let verb = match verb {
            "linked" => "links",
            "retargeted" => "retargets",
            "removed" => "removes",
            "mirrored" => "mirrors",
            other => panic!("sync printed {other}"),
        };
        assert_line(&out, &format!("{path}: the next `skiman sync` {verb} it"));
    }
    for line in String::from_utf8(sync.stderr).unwrap().lines() {
        let said = line
            .strip_prefix("warning: ")
            .or_else(|| line.strip_prefix("error: "));
        assert_line(&out, said.unwrap_or(line));
    }
    out
}

/// doctor runs only read-only git queries, which take no optional lock: a
/// staged mirror file whose time changed leaves the index as it is.
#[test]
fn doctor_changes_nothing_on_disk_not_even_the_git_index() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let work = sandbox.home().join("work");
    write(work.join("a/AGENTS.md"), "rules\n");
    let add = ["project", "add", "--id", "work"];
    sandbox.skiman().args(add).arg(&work).assert().code(0);
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    let repo = sandbox.canon();
    sandbox.git(&repo, ["add", "projects"]);
    let mirror = repo.join("projects/work/a/AGENTS.md");
    sandbox
        .command("touch")
        .args(["-t", "203001010000"])
        .arg(&mirror)
        .assert()
        .success();
    // A manual that is out of date, and an item both sides changed alike
    // (its synced hash is due): each write sync makes, a plan must skip.
    write(work.join("b/AGENTS.md"), "v1\n");
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    write(work.join("b/AGENTS.md"), "v2\n");
    write(repo.join("projects/work/b/AGENTS.md"), "v2\n");
    write(repo.join(".skiman/skiman-manual/SKILL.md"), "out of date\n");
    let before = on_disk(&sandbox);

    doctor(&sandbox, 0);

    assert_eq!(on_disk(&sandbox), before);
}

/// A plan sees what blocks a write before any write: `linked/` holds
/// another spelling of a linked folder's id, so the id is occupied (on a
/// file system that ignores case; elsewhere the two names differ).
#[test]
fn doctor_foresees_sync_when_linked_holds_another_spelling_of_an_id() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let probe = sandbox.root().join("Case");
    write(&probe, "");
    if std::fs::symlink_metadata(sandbox.root().join("case")).is_err() {
        return;
    }
    skill_folder(sandbox.home().join("outside/notes"), "notes");
    let manifest = sandbox.canon().join("skiman.toml");
    let text = format!(
        "{}\n[linked.notes]\npath = \"~/outside\"\n",
        read(&manifest)
    );
    write(&manifest, text);
    std::fs::create_dir_all(sandbox.canon().join("linked/NOTES")).unwrap();

    let out = assert_doctor_foresees_sync(&sandbox);

    assert!(!out.contains("linked/notes: the next"), "{out}");
}

/// A tool directory that is a link leading nowhere, or round in a circle,
/// or whose parent is a link leading nowhere: sync and doctor both say it
/// cannot be used (exit 4), neither calls it missing.
#[test]
fn doctor_foresees_sync_when_a_tool_directory_is_a_broken_link() {
    for (tool, link, target) in [
        (Tool::ClaudeCode, ".claude/skills", "nowhere"),
        (Tool::ClaudeCode, ".claude/skills", "skills"),
        (Tool::Codex, ".agents", "nowhere"),
    ] {
        let sandbox = Sandbox::new();
        init(&sandbox, "none");
        let path = sandbox.home().join(link);
        std::fs::remove_dir_all(&path).unwrap();
        std::os::unix::fs::symlink(target, &path).unwrap();

        let out = assert_doctor_foresees_sync(&sandbox);

        let dir = sandbox.tool_dir(tool).display().to_string();
        assert!(
            out.contains(&format!("cannot use {dir} as a tool directory")),
            "{link}: {out}"
        );
        assert!(
            !out.contains("(missing; the next sync creates it)"),
            "{link}: {out}"
        );
    }
}

/// The manual's folder is a link: sync cannot write the manual (exit 4),
/// and doctor says so.
#[test]
fn doctor_foresees_sync_when_the_manual_folder_is_a_link() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let manual = sandbox.canon().join(".skiman/skiman-manual");
    std::fs::remove_dir_all(&manual).unwrap();
    let elsewhere = sandbox.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &manual).unwrap();

    let out = assert_doctor_foresees_sync(&sandbox);

    assert!(
        out.contains(&format!("cannot write skiman-manual: {}", manual.display())),
        "{out}"
    );
}

/// `linked/` and a tool directory are missing: doctor names each link the
/// next sync makes in them, as if they were made.
#[test]
fn doctor_foresees_sync_in_folders_sync_makes() {
    let sandbox = Sandbox::builder()
        .tool(Tool::Codex, ToolSetup::PresenceOnly)
        .build();
    init(&sandbox, "none");
    let dir = sandbox.tool_dir(Tool::Codex);
    let _ = std::fs::remove_dir_all(&dir);
    skill_folder(sandbox.home().join("outside/notes"), "notes");
    let manifest = sandbox.canon().join("skiman.toml");
    let text = format!(
        "{}\n[linked.notes]\npath = \"~/outside\"\n",
        read(&manifest)
    );
    write(&manifest, text);
    let _ = std::fs::remove_dir_all(sandbox.canon().join("linked"));

    let out = assert_doctor_foresees_sync(&sandbox);

    let linked = sandbox.canon().join("linked/notes");
    assert_line(
        &out,
        &format!("{}: the next `skiman sync` links it", linked.display()),
    );
    let notes = dir.join("notes");
    assert_line(
        &out,
        &format!("{}: the next `skiman sync` links it", notes.display()),
    );
}

/// A tool directory that is a link to another tool's tool directory, as on
/// the repo owner's machine (`~/.gemini/config/skills -> ~/.agents/skills`,
/// `docs/phase1-results.md:45`), once the folder it points to is removed by
/// hand (`removed`), or the other way round (`reverse`: `~/.agents/skills ->
/// ~/.claude/skills`): sync makes the folder for the earlier tool and serves
/// both through it, so doctor names those links, not a link that leads
/// nowhere.
#[test]
fn doctor_foresees_sync_when_a_tool_directory_links_to_another_tools_missing_one() {
    for case in ["removed", "reverse"] {
        let sandbox = Sandbox::new();
        let (target, alias) = match case {
            "removed" => (Tool::Codex, Tool::Antigravity),
            _ => (Tool::ClaudeCode, Tool::Codex),
        };
        let (target, alias) = (sandbox.tool_dir(target), sandbox.tool_dir(alias));
        let _ = std::fs::remove_dir_all(&alias);
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        init(&sandbox, "none");
        skill_folder(sandbox.canon().join("skills/notes"), "notes");
        sandbox.skiman().arg("sync").assert().code(0);
        std::fs::remove_dir_all(&target).unwrap();

        let out = assert_doctor_foresees_sync(&sandbox);

        assert!(!out.contains("leads nowhere"), "{case}: {out}");
        let notes = target.join("notes");
        let line = format!("{}: the next `skiman sync` links it", notes.display());
        assert!(out.lines().any(|l| l == line), "{case}: {out}");
    }
}

/// A link above a tool directory into another tool's: Antigravity's
/// `~/.gemini/config -> ~/.agents` once `~/.agents` is removed by hand
/// (`gone`), its tool directory a link to that folder (`above`,
/// `~/.gemini/config/skills -> ~/.agents`), `~/.gemini/config ->
/// ~/.agents/skills` once that is removed (`inside`), or a chain through a
/// dotfiles folder, `~/.gemini/config -> ~/dotfiles/shared -> ~/.agents`,
/// spelled absolute (`chain`) or relative (`chain-relative`).
/// Sync makes `~/.agents/skills` for Codex first and then serves Antigravity
/// through the links, so doctor names those links. A link to a folder beside
/// the one sync makes (`sibling`, `~/.gemini/config/skills ->
/// ~/.agents/skills-gemini`; `sibling-upper`, `~/.gemini/config ->
/// ~/.agents/gemini`) still leads nowhere after it, also through the dotfiles
/// folder (`chain-sibling`, `~/.gemini/config/skills ->
/// ~/dotfiles/shared/skills-gemini`; `chain-sibling-upper`, `~/.gemini/config
/// -> ~/dotfiles/shared/gemini`), and so does Codex's the
/// other way round (`reverse`, `~/.agents -> ~/.gemini/config` once that is
/// removed): both say so.
#[test]
fn doctor_foresees_sync_when_a_link_above_a_tool_directory_leads_to_another_tools_missing_one() {
    for case in [
        "gone",
        "above",
        "inside",
        "chain",
        "chain-relative",
        "sibling",
        "sibling-upper",
        "chain-sibling",
        "chain-sibling-upper",
        "reverse",
    ] {
        let sandbox = Sandbox::new();
        let home = sandbox.home();
        let (agents, config) = (home.join(".agents"), home.join(".gemini/config"));
        let shared = home.join("dotfiles/shared");
        let (link, to, removed) = match case {
            "gone" => (config.clone(), agents.clone(), agents.clone()),
            "above" => (config.join("skills"), agents.clone(), agents.clone()),
            "inside" => (config.clone(), agents.join("skills"), agents.join("skills")),
            "chain" | "chain-relative" => (config.clone(), shared.clone(), agents.clone()),
            "sibling" => (
                config.join("skills"),
                agents.join("skills-gemini"),
                agents.clone(),
            ),
            "sibling-upper" => (config.clone(), agents.join("gemini"), agents.clone()),
            "chain-sibling" => (
                config.join("skills"),
                shared.join("skills-gemini"),
                agents.clone(),
            ),
            "chain-sibling-upper" => (config.clone(), shared.join("gemini"), agents.clone()),
            _ => (agents.clone(), config.clone(), config.clone()),
        };
        let _ = std::fs::remove_dir_all(&link);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        if case.starts_with("chain") {
            std::fs::create_dir_all(agents.join(to.strip_prefix(&shared).unwrap())).unwrap();
            std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
            let (via, there) = match case {
                "chain-relative" => ("../.agents".into(), "../dotfiles/shared".into()),
                _ => (agents.clone(), to.clone()),
            };
            std::os::unix::fs::symlink(via, &shared).unwrap();
            std::os::unix::fs::symlink(there, &link).unwrap();
        } else {
            std::fs::create_dir_all(&to).unwrap();
            std::os::unix::fs::symlink(&to, &link).unwrap();
        }
        init(&sandbox, "none");
        skill_folder(sandbox.canon().join("skills/notes"), "notes");
        sandbox.skiman().arg("sync").assert().code(0);
        std::fs::remove_dir_all(&removed).unwrap();

        let out = assert_doctor_foresees_sync(&sandbox);

        let (served, nowhere) = match case {
            "gone" | "chain" | "chain-relative" => (agents.join("skills/notes"), false),
            "above" | "inside" => (config.join("skills/notes"), false),
            "sibling" | "sibling-upper" | "chain-sibling" | "chain-sibling-upper" => {
                (agents.join("skills/notes"), true)
            }
            _ => (config.join("skills/notes"), true),
        };
        assert_eq!(out.contains("leads nowhere"), nowhere, "{case}: {out}");
        let line = format!("{}: the next `skiman sync` links it", served.display());
        assert!(out.lines().any(|l| l == line), "{case}: {out}");
    }
}

/// A tool directory behind a link that is a file, the link leading where an
/// earlier tool's folder is: `~/.agents -> ~/dotfiles` beside `~/.claude ->
/// ~/dotfiles/claude` with `~/dotfiles/skills` a file (`dotfiles`), or
/// `~/.gemini/config -> ~/.agents/skills` with `~/.agents/skills/skills` a
/// file (`inside`). Nothing sync makes fixes it, so
/// doctor says sync's own error, not one line per skill.
#[test]
fn doctor_says_what_sync_says_when_a_tool_directory_behind_a_link_is_a_file() {
    for case in ["dotfiles", "inside"] {
        let sandbox = Sandbox::new();
        let home = sandbox.home();
        let dotfiles = home.join("dotfiles");
        let (link, to, dir) = match case {
            "dotfiles" => (
                home.join(".agents"),
                dotfiles.clone(),
                sandbox.tool_dir(Tool::Codex),
            ),
            _ => (
                home.join(".gemini/config"),
                sandbox.tool_dir(Tool::Codex),
                sandbox.tool_dir(Tool::Antigravity),
            ),
        };
        if case == "dotfiles" {
            let claude = sandbox.presence_dir(Tool::ClaudeCode);
            std::fs::create_dir_all(dotfiles.join("claude/skills")).unwrap();
            std::fs::remove_dir_all(&claude).unwrap();
            std::os::unix::fs::symlink(dotfiles.join("claude"), &claude).unwrap();
        }
        let _ = std::fs::remove_dir_all(&link);
        std::fs::create_dir_all(&to).unwrap();
        std::os::unix::fs::symlink(&to, &link).unwrap();
        init(&sandbox, "none");
        skill_folder(sandbox.canon().join("skills/notes"), "notes");
        sandbox.skiman().arg("sync").assert().code(0);
        let file = dir.canonicalize().unwrap();
        std::fs::remove_dir_all(&file).unwrap();
        std::fs::write(&file, "not a folder\n").unwrap();

        let out = assert_doctor_foresees_sync(&sandbox);

        let said = format!("cannot use {} as a tool directory: ", dir.display());
        assert!(out.lines().any(|l| l.starts_with(&said)), "{case}: {out}");
        assert!(!out.contains("cannot inspect"), "{case}: {out}");
    }
}

/// Sync cannot write the manual when its `SKILL.md` is a folder: doctor
/// says the same error (exit 4), and links only what sync links.
#[test]
fn doctor_foresees_sync_when_the_manual_file_is_a_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let file = sandbox.canon().join(".skiman/skiman-manual/SKILL.md");
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();

    let out = assert_doctor_foresees_sync(&sandbox);

    assert!(
        out.contains(&format!("{} is a folder", file.display())),
        "{out}"
    );
}

/// `.skiman/swap` is a file of the user's, so sync cannot stage a mirror
/// write: doctor says the same error (exit 4), not that the
/// next sync mirrors the item.
#[test]
fn doctor_foresees_sync_when_the_mirror_staging_is_blocked() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let work = sandbox.home().join("work");
    write(work.join("a/AGENTS.md"), "v1\n");
    let add = ["project", "add", "--id", "work"];
    sandbox.skiman().args(add).arg(&work).assert().code(0);
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    write(work.join("a/AGENTS.md"), "v2\n");
    let swap = sandbox.canon().join(".skiman/swap");
    let _ = std::fs::remove_dir_all(&swap);
    write(&swap, "mine\n");

    let out = assert_doctor_foresees_sync(&sandbox);

    assert!(out.contains("cannot create a folder in"), "{out}");
}

/// The manual's `SKILL.md` is a link to a folder. Only `SKILL.md`
/// itself as a folder blocks the write; the link is replaced, as before, and the folder it led to keeps its content. doctor and sync agree.
#[test]
fn doctor_foresees_sync_when_the_manual_file_is_a_link_to_a_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let file = sandbox.canon().join(".skiman/skiman-manual/SKILL.md");
    let keep = sandbox.home().join("keep");
    write(keep.join("data"), "keep this");
    std::fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(&keep, &file).unwrap();

    let out = assert_doctor_foresees_sync(&sandbox);

    assert!(!out.contains("is a folder"), "{out}");
    assert!(!std::fs::symlink_metadata(&file).unwrap().is_symlink());
    assert!(read(&file).contains("name: skiman-manual"));
    assert_eq!(read(keep.join("data")), "keep this");
}

/// The manual's `SKILL.md` is a FIFO nothing writes to, then a link to
/// `/dev/zero`. The check neither waits for a writer nor reads on and on:
/// doctor ends (exit 0, foreseeing nothing), and sync replaces it with the
/// manual.
#[test]
fn doctor_and_sync_do_not_wait_on_a_manual_fifo() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let file = sandbox.canon().join(".skiman/skiman-manual/SKILL.md");
    std::fs::remove_file(&file).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&file)
        .status()
        .unwrap();
    assert!(made.success());
    let quick = std::time::Duration::from_secs(10);

    let doctor = sandbox
        .skiman()
        .arg("doctor")
        .timeout(quick)
        .output()
        .unwrap();
    let sync = sandbox
        .skiman()
        .arg("sync")
        .timeout(quick)
        .output()
        .unwrap();

    assert_eq!(doctor.status.code(), Some(0), "{doctor:?}");
    assert_eq!(sync.status.code(), Some(0), "{sync:?}");
    assert!(read(&file).contains("name: skiman-manual"));

    // A link to `/dev/zero` there: the check reads one byte past the manual, no more.
    std::fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink("/dev/zero", &file).unwrap();
    let doctor = sandbox
        .skiman()
        .arg("doctor")
        .timeout(quick)
        .output()
        .unwrap();
    let sync = sandbox
        .skiman()
        .arg("sync")
        .timeout(quick)
        .output()
        .unwrap();
    assert_eq!(doctor.status.code(), Some(0), "{doctor:?}");
    assert_eq!(sync.status.code(), Some(0), "{sync:?}");
    assert!(read(&file).contains("name: skiman-manual"));
}

/// `~/.claude` is a link into the canon's git folder, so the tool directory
/// sync would make lands there: refused by sync and doctor alike, and
/// doctor's tool line does not say it is made.
#[test]
fn doctor_foresees_sync_when_a_missing_tool_directory_is_in_the_canons_git_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox, "none");
    let claude = sandbox.presence_dir(Tool::ClaudeCode);
    std::fs::remove_dir_all(&claude).unwrap();
    std::os::unix::fs::symlink(sandbox.canon().join(".git/hooks"), &claude).unwrap();

    let out = assert_doctor_foresees_sync(&sandbox);

    assert!(out.contains("inside the canon"), "{out}");
    assert!(!out.contains("next sync creates"), "{out}");
}
