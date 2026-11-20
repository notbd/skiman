//! `skiman import <folder>`: bring an existing setup in from a prepared
//! import folder (spec Import, ADR 0001).

use std::collections::BTreeMap;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::support::{
    Sandbox, Setup, Tool, backup_dirs, fingerprint, import_killed_at, import_with_hook, init, read,
    setup, skill_md, tree, write,
};

/// An import folder outside the canon holding own skill `notes`, a
/// shared body and a `skiman.toml` with `manifest` as its text.
fn import_folder(sandbox: &Sandbox, manifest: &str) -> PathBuf {
    let folder = sandbox.root().join("import");
    write(folder.join("skills/notes/SKILL.md"), skill_md("notes"));
    write(
        folder.join("instructions/shared.md"),
        "# Shared\n\nBe brief.\n",
    );
    write(folder.join("skiman.toml"), manifest);
    folder
}

const TOOLS: &str = "[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n";

/// The whole canon, every tool directory and instruction file, and the
/// machine state except the git cache (fetching may fill it), to prove a
/// command changed nothing.
fn world(sandbox: &Sandbox) -> Vec<BTreeMap<PathBuf, String>> {
    let mut state = tree(sandbox.state_home());
    state.retain(|path, _| path != Path::new("skiman") && !path.starts_with("skiman/cache"));
    let mut world = vec![tree(sandbox.canon()), state];
    world.extend(Tool::ALL.iter().map(|t| tree(sandbox.presence_dir(*t))));
    world.push(tree(sandbox.home().join(".agents")));
    world
}

#[test]
fn without_a_terminal_import_needs_yes() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let folder = import_folder(&sandbox, TOOLS);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .assert()
        .code(2)
        .stdout_eq("")
        .stderr_eq(
            "error: missing flag without a terminal: --yes (import without asking for confirmation)\n",
        );
    assert_eq!(world(&sandbox), before);
}

/// A messy setup: instruction files that are regular files, a skill copied
/// into one tool directory and linked from another, a copy the import
/// leaves out, a managed-elsewhere folder, a non-skill file at a skill's
/// name, and an upstream `acme/skills` holding `pdf` with a `.gitignore`.
struct Messy {
    folder: PathBuf,
    commit: String,
}

fn messy(sandbox: &Sandbox) -> Messy {
    let home = sandbox.home();
    write(
        sandbox.instruction_file(Tool::ClaudeCode),
        "old claude rules\n",
    );
    write(sandbox.instruction_file(Tool::Codex), "old codex rules\n");
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write(claude.join("notes/SKILL.md"), skill_md("notes"));
    write(claude.join("old-copy/SKILL.md"), skill_md("old-copy"));
    write(claude.join("synced/web/SKILL.md"), skill_md("web"));
    std::fs::create_dir_all(sandbox.tool_dir(Tool::Codex)).unwrap();
    std::os::unix::fs::symlink(
        claude.join("notes"),
        sandbox.tool_dir(Tool::Codex).join("notes"),
    )
    .unwrap();
    write(
        sandbox.tool_dir(Tool::Antigravity).join("notes"),
        "not a skill\n",
    );
    let _ = home;

    let up = sandbox.upstream("acme/skills");
    up.write("skills/pdf/SKILL.md", skill_md("pdf"))
        .write("skills/pdf/reference.md", "# Reference\n")
        .write("skills/pdf/.gitignore", "*.tmp\n");
    let commit = up.commit("v1");
    up.write("skills/pdf/reference.md", "# Reference v2\n");
    up.commit("v2");

    let folder = import_folder(
        sandbox,
        &format!(
            "# Imported setup\n[skills.pdf.source]\nurl = \"https://github.com/acme/skills.git\"\npath = \"skills/pdf\"\nref = \"{commit}\"\n"
        ),
    );
    write(
        folder.join("skills/pdf/SKILL.md"),
        format!("{}\nMy local note.\n", skill_md("pdf")),
    );
    write(folder.join("skills/pdf/reference.md"), "# Reference\n");
    Messy { folder, commit }
}

#[test]
fn import_brings_a_messy_setup_in_and_keeps_every_original_in_the_backup() {
    let sandbox = Sandbox::new();
    let Messy { folder, commit } = messy(&sandbox);
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.canon())
        .assert()
        .code(3);
    let repo = sandbox.canon();
    write(repo.join("instructions/shared.md"), "old shared body\n");
    let old_manifest = read(repo.join("skiman.toml"));
    let folder_before = tree(&folder);
    let synced_before = tree(sandbox.tool_dir(Tool::ClaudeCode).join("synced"));

    let output = sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(3)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    let codex = sandbox.tool_dir(Tool::Codex);
    let gemini = sandbox.tool_dir(Tool::Antigravity);
    let preview = format!(
        "import {folder} into the canon {repo}:
  add skill notes (own)
  add skill pdf (third-party, from https://github.com/acme/skills.git (skills/pdf) at {commit} (commit)); files that differ from upstream:
      modified SKILL.md
      .gitignore  [not installed]
  move {repo}/skiman.toml to the backup, then write the import's
  move {repo}/instructions/shared.md to the backup, then write the import's
  move {claude}/old-copy to the backup (no imported skill of that name is distributed)
  move {codex}/notes to the backup, then link it to skill notes
  move {claude}/notes to the backup, then link it to skill notes
  move {home}/.claude/CLAUDE.md to the backup, then link it to the shared body
  move {home}/.codex/AGENTS.md to the backup, then link it to the shared body
  skip {claude}/synced: managed elsewhere
  keep {gemini}/notes: it is not a skill, so skill notes is not linked there
",
        folder = folder.display(),
        repo = repo.display(),
        claude = claude.display(),
        codex = codex.display(),
        gemini = gemini.display(),
        home = sandbox.home().display(),
    );
    assert!(stdout.starts_with(&preview), "{stdout}");
    let backup = stdout
        .lines()
        .find_map(|l| {
            l.strip_prefix("imported ")
                .and_then(|l| l.split_once("; the originals this run moved are in "))
        })
        .map(|(_, b)| PathBuf::from(b))
        .unwrap_or_else(|| panic!("no backup line in {stdout}"));
    assert!(backup.starts_with(sandbox.state_home().join("skiman/backups")));
    assert!(
        stdout.ends_with(&format!(
            "the import folder {} can be deleted\n",
            folder.display()
        )),
        "{stdout}"
    );
    assert!(
        stderr.contains(&format!("{}/notes is occupied", gemini.display())),
        "{stderr}"
    );

    // The canon holds the import.
    assert_eq!(
        read(repo.join("skills/pdf/SKILL.md")),
        read(folder.join("skills/pdf/SKILL.md"))
    );
    assert_eq!(read(repo.join("upstream/pdf/SKILL.md")), skill_md("pdf"));
    assert_eq!(
        read(repo.join("upstream/pdf/reference.md")),
        "# Reference\n"
    );
    assert_eq!(
        read(repo.join("instructions/shared.md")),
        "# Shared\n\nBe brief.\n"
    );
    let manifest = read(repo.join("skiman.toml"));
    assert!(
        manifest.starts_with("[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n"),
        "{manifest}"
    );
    assert!(manifest.contains("ref_kind = \"commit\""), "{manifest}");
    let lock = read(repo.join("skiman.lock"));
    assert!(lock.contains(&format!("commit = \"{commit}\"")), "{lock}");
    assert!(lock.contains(&format!("ref = \"{commit}\"")), "{lock}");
    sandbox.skiman().arg("check").assert().code(0);

    // Every original is in the backup, replaced by an owned link.
    assert_eq!(
        read(backup.join("repo/instructions/shared.md")),
        "old shared body\n"
    );
    assert_eq!(read(backup.join("repo/skiman.toml")), old_manifest);
    assert_eq!(
        read(backup.join("claude-code/CLAUDE.md")),
        "old claude rules\n"
    );
    assert_eq!(read(backup.join("codex/AGENTS.md")), "old codex rules\n");
    assert_eq!(
        read(backup.join("claude-code/notes/SKILL.md")),
        skill_md("notes")
    );
    assert_eq!(
        read(backup.join("claude-code/old-copy/SKILL.md")),
        skill_md("old-copy")
    );
    assert_eq!(
        std::fs::read_link(backup.join("codex/notes")).unwrap(),
        claude.join("notes")
    );
    for dir in [&claude, &codex] {
        crate::support::assert_symlink(
            dir.join("notes"),
            repo.join("skills/notes"),
            repo.join("skills/notes"),
        );
        crate::support::assert_symlink(
            dir.join("pdf"),
            repo.join("skills/pdf"),
            repo.join("skills/pdf"),
        );
    }
    assert!(std::fs::symlink_metadata(claude.join("old-copy")).is_err());
    for tool in [Tool::ClaudeCode, Tool::Codex, Tool::Antigravity] {
        let shared = repo.join("instructions/shared.md");
        crate::support::assert_symlink(sandbox.instruction_file(tool), &shared, &shared);
    }
    // Left alone: managed elsewhere, and the non-skill entry.
    assert_eq!(tree(claude.join("synced")), synced_before);
    assert_eq!(read(gemini.join("notes")), "not a skill\n");
    assert_eq!(tree(&folder), folder_before);

    assert!(!sandbox.state_home().join("skiman/import").exists());
}

/// Every case leaves everything unchanged: the import folder, the canon,
/// the tool directories, the instruction files and machine state.
#[track_caller]
fn assert_refused(sandbox: &Sandbox, folder: &Path, code: i32, stderr_has: &[&str]) -> String {
    let before = world(sandbox);
    let folder_before = tree(folder);
    let output = sandbox
        .skiman()
        .arg("import")
        .arg(folder)
        .arg("--yes")
        .assert()
        .code(code)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    for text in stderr_has {
        assert!(stderr.contains(text), "{text:?} not in {stderr}");
    }
    assert_eq!(tree(folder), folder_before);
    let after = world(sandbox);
    for (b, a) in before.iter().zip(&after) {
        let changed: Vec<_> = a
            .iter()
            .filter(|(k, v)| b.get(*k) != Some(v))
            .map(|(k, _)| k)
            .chain(b.keys().filter(|k| !a.contains_key(*k)))
            .collect();
        assert!(changed.is_empty(), "changed: {changed:?}\n{stderr}");
    }
    stderr
}

#[test]
fn a_canon_with_a_skill_a_snapshot_or_a_lock_entry_refuses_import() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    let folder = import_folder(&sandbox, TOOLS);
    write(repo.join("skills/.hidden/x"), "dot names do not count\n");
    write(repo.join("upstream/.keep"), "");

    write(repo.join("skills/mine/SKILL.md"), skill_md("mine"));
    assert_refused(
        &sandbox,
        &folder,
        1,
        &[
            "  skills/mine is not in the import folder: remove it from the canon, or put it in the import folder\n",
        ],
    );
    std::fs::remove_dir_all(repo.join("skills/mine")).unwrap();

    write(repo.join("upstream/pdf/SKILL.md"), skill_md("pdf"));
    assert_refused(
        &sandbox,
        &folder,
        1,
        &[
            "  upstream/pdf is the snapshot of no third-party skill in the import folder: remove it from the canon, or put pdf in the import folder as a third-party skill\n",
        ],
    );
    std::fs::remove_dir_all(repo.join("upstream/pdf")).unwrap();

    write(
        repo.join("skiman.lock"),
        "version = 1\n\n[skills.pdf]\nsnapshot = \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"\n",
    );
    assert_refused(
        &sandbox,
        &folder,
        1,
        &[
            "  skiman.lock has an entry for pdf, which is no third-party skill in the import folder: remove the entry from the canon's skiman.lock, or put pdf in the import folder as a third-party skill\n",
        ],
    );
    std::fs::remove_file(repo.join("skiman.lock")).unwrap();

    // The import's own skill, already in the canon as the import folder
    // holds it, is kept, and the rest is written.
    write(repo.join("skills/notes/SKILL.md"), skill_md("notes"));
    let output = sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\n  keep skill notes (own)\n"), "{stdout}");
    assert_eq!(
        read(repo.join("instructions/shared.md")),
        "# Shared\n\nBe brief.\n"
    );
    assert_eq!(read(repo.join("skiman.toml")), TOOLS);
}

#[test]
fn a_folder_that_breaks_a_rule_is_refused_as_a_whole_naming_file_and_line() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let folder = import_folder(&sandbox, TOOLS);
    write(folder.join("skills/good/SKILL.md"), skill_md("good"));
    write(
        folder.join("skills/bad/SKILL.md"),
        "---\nname: bad\ndescription: d\ndescription: twice\n---\n",
    );
    write(
        folder.join("instructions/shared.md"),
        "Read this:\n@notes.md\n",
    );
    assert_refused(
        &sandbox,
        &folder,
        1,
        &[
            "skills/bad/SKILL.md:4: duplicate-key",
            "instructions/shared.md:2: at-import",
        ],
    );

    // Anything besides the three entries is refused too.
    write(folder.join("instructions/shared.md"), "Fine.\n");
    std::fs::remove_dir_all(folder.join("skills/bad")).unwrap();
    write(folder.join("skiman.lock"), "version = 1\n");
    assert_refused(
        &sandbox,
        &folder,
        1,
        &["skiman.lock is not part of an import folder"],
    );
    std::fs::remove_file(folder.join("skiman.lock")).unwrap();

    // A source table without its skill folder fails Consistency once fetched.
    let up = sandbox.upstream("acme/skills");
    up.write("SKILL.md", skill_md("pdf"));
    let commit = up.commit("v1");
    write(
        folder.join("skiman.toml"),
        format!(
            "{TOOLS}\n[skills.pdf.source]\nurl = \"https://github.com/acme/skills.git\"\npath = \"\"\nref = \"{commit}\"\n"
        ),
    );
    assert_refused(&sandbox, &folder, 1, &["skiman.toml:4: leftover-record"]);
}

#[test]
fn a_secret_in_a_declared_upstream_refuses_the_import_until_it_is_exempted() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let key = format!("{}{}", "AKIA", "ABCDEFGHIJKLMNOP");
    let up = sandbox.upstream("acme/skills");
    up.write("SKILL.md", skill_md("pdf"))
        .write("tests/fixture.env", format!("KEY={key}\n"));
    let commit = up.commit("v1");
    let manifest = format!(
        "{TOOLS}\n[skills.pdf.source]\nurl = \"https://github.com/acme/skills.git\"\npath = \"\"\nref = \"{commit}\"\n"
    );
    let folder = import_folder(&sandbox, &manifest);
    write(folder.join("skills/pdf/SKILL.md"), skill_md("pdf"));

    assert_refused(
        &sandbox,
        &folder,
        1,
        &[
            "upstream/pdf/tests/fixture.env:1: aws-access-key",
            "[[secrets.allow]]\npath = \"skills/pdf/tests/fixture.env\"",
        ],
    );
    let output = sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains(&key), "{stderr}");
    let entry = &stderr[stderr.find("[[secrets.allow]]").unwrap()..];
    write(folder.join("skiman.toml"), format!("{manifest}\n{entry}"));

    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);
    assert_eq!(
        read(sandbox.canon().join("upstream/pdf/tests/fixture.env")),
        format!("KEY={key}\n")
    );
}

#[test]
fn a_special_file_in_an_entry_to_move_refuses_the_import_naming_it() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let entry = sandbox.tool_dir(Tool::ClaudeCode).join("notes");
    write(entry.join("SKILL.md"), skill_md("notes"));
    let fifo = entry.join("pipe");
    sandbox
        .std_command("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success()
        .then_some(())
        .unwrap();
    let folder = import_folder(&sandbox, TOOLS);
    let repo_before = tree(sandbox.canon());
    let folder_before = tree(&folder);

    let output = sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(fifo.to_str().unwrap()), "{stderr}");
    assert!(stderr.contains("special files"), "{stderr}");
    assert!(
        std::fs::symlink_metadata(&fifo)
            .unwrap()
            .file_type()
            .is_fifo()
    );
    assert!(entry.join("SKILL.md").is_file());
    assert_eq!(tree(sandbox.canon()), repo_before);
    assert_eq!(tree(&folder), folder_before);
    assert!(!sandbox.state_home().join("skiman/import").exists());
    assert!(!sandbox.state_home().join("skiman/backups").exists());
}

/// A tool directory leading into the canon's `skills/`: running the import
/// again would take the canon's own skills for a tool's and move them to
/// the backup, so it is refused as sync's guard refuses writing there.
#[test]
fn a_tool_directory_that_leads_into_the_canon_refuses_the_import() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let folder = import_folder(&sandbox, TOOLS);
    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    std::fs::remove_dir_all(&claude).unwrap();
    std::os::unix::fs::symlink(sandbox.canon().join("skills"), &claude).unwrap();

    assert_refused(
        &sandbox,
        &folder,
        1,
        &["inside the canon, so skiman writes nothing there"],
    );
    assert_eq!(
        read(sandbox.canon().join("skills/notes/SKILL.md")),
        skill_md("notes")
    );
}

/// Codex's tool directory leads through `bundle`, a foreign skill the
/// import moves: once moved, the tool directory would lead nowhere.
#[test]
fn a_tool_directory_through_an_entry_the_import_moves_refuses_the_import() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let bundle = sandbox.root().join("real-bundle");
    write(bundle.join("SKILL.md"), skill_md("bundle"));
    write(bundle.join("skills/pdf/SKILL.md"), skill_md("pdf"));
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    std::fs::create_dir_all(&claude).unwrap();
    std::os::unix::fs::symlink(&bundle, claude.join("bundle")).unwrap();
    let codex = sandbox.tool_dir(Tool::Codex);
    std::fs::remove_dir_all(&codex).unwrap();
    std::os::unix::fs::symlink(claude.join("bundle/skills"), &codex).unwrap();
    let folder = import_folder(&sandbox, TOOLS);

    let through = format!(
        "  {}: through {}",
        codex.display(),
        claude.join("bundle").display()
    );
    assert_refused(&sandbox, &folder, 1, &[&through]);
}

/// A backup folder that cannot be made stops the import before the canon
/// is written; once it can be made, the same command finishes.
#[test]
fn a_backup_folder_that_cannot_be_made_stops_the_import_before_anything_is_written() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let original = sandbox.tool_dir(Tool::ClaudeCode).join("old");
    write(original.join("SKILL.md"), skill_md("old"));
    let backups = sandbox.state_home().join("skiman/backups");
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o555)).unwrap();
    // The canon's own skiman.toml and shared body: writing them backs nothing up.
    let folder = import_folder(&sandbox, &read(sandbox.canon().join("skiman.toml")));
    let body = read(sandbox.canon().join("instructions/shared.md"));
    write(folder.join("instructions/shared.md"), body);

    assert_refused(&sandbox, &folder, 4, &["nothing was changed"]);
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o755)).unwrap();
    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);
    assert!(!original.exists());
    assert_eq!(
        read(sandbox.canon().join("skills/notes/SKILL.md")),
        skill_md("notes")
    );
}

/// A canon `skiman.lock` that does not parse is reported before any
/// upstream is fetched: here the upstream does not exist, and fetching it
/// would fail first.
#[test]
fn a_broken_canon_lock_refuses_the_import_before_anything_is_fetched() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(sandbox.canon().join("skiman.lock"), "version = [\n");
    let folder = import_folder(
        &sandbox,
        &format!(
            "{TOOLS}\n[skills.pdf.source]\nurl = \"https://github.com/acme/skills.git\"\npath = \"\"\n"
        ),
    );
    write(folder.join("skills/pdf/SKILL.md"), skill_md("pdf"));

    assert_refused(&sandbox, &folder, 1, &["skiman.lock: TOML parse error"]);
}

/// Running the import again with one more skill rewrites `skiman.toml`; a
/// skill the folder gives no table keeps what the canon records about it,
/// and one the folder gives a table keeps the folder's table alone.
#[test]
fn import_again_keeps_the_summary_origin_and_install_day_of_a_skill_without_a_table() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let folder = import_folder(&sandbox, TOOLS);
    write(folder.join("skills/more/SKILL.md"), skill_md("more"));
    let import = || {
        sandbox
            .skiman()
            .arg("import")
            .arg(&folder)
            .arg("--yes")
            .assert()
            .code(0);
    };
    import();
    let manifest = sandbox.canon().join("skiman.toml");
    let notes = "[skills.notes]\nsummary = \"Keep notes\"\norigin = \"written here\"\ninstalled = 2026-11-14\n";
    let canon_more = "[skills.more]\nenabled = false\nsummary = \"The canon's\"\n";
    write(
        &manifest,
        format!("{}\n{notes}\n{canon_more}", read(&manifest)),
    );
    let folder_more = "[skills.more]\nsummary = \"The folder's\"\n";
    write(
        folder.join("skiman.toml"),
        format!("{TOOLS}\n{folder_more}"),
    );
    write(folder.join("skills/extra/SKILL.md"), skill_md("extra"));

    import();
    let written = read(&manifest);
    assert!(written.contains(notes), "{written}");
    assert!(written.contains(folder_more), "{written}");
    assert!(!written.contains("The canon's"), "{written}");
    assert!(!written.contains("enabled = false"), "{written}");
}

#[test]
fn a_canon_whose_shared_body_or_skiman_toml_is_a_link_refuses_the_import_before_anything_is_written()
 {
    for rel in ["instructions/shared.md", "skiman.toml"] {
        let sandbox = Sandbox::new();
        init(&sandbox);
        let repo = sandbox.canon();
        let elsewhere = sandbox.home().join("elsewhere");
        std::fs::rename(repo.join(rel), &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, repo.join(rel)).unwrap();
        let folder = import_folder(&sandbox, TOOLS);

        let lines = [
            "holds a link, folder or special file where import needs a regular file",
            &format!("  {rel} is not a regular file\n"),
        ];
        let first = assert_refused(&sandbox, &folder, 1, &lines);
        let again = assert_refused(&sandbox, &folder, 1, &lines);
        assert_eq!(again, first);
    }
}

#[test]
fn with_a_terminal_declining_changes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(
        sandbox.tool_dir(Tool::ClaudeCode).join("notes/SKILL.md"),
        skill_md("notes"),
    );
    let folder = import_folder(&sandbox, TOOLS);
    let before = world(&sandbox);
    let folder_before = tree(&folder);

    let mut pty = sandbox.skiman_pty([std::ffi::OsStr::new("import"), folder.as_os_str()]);
    let preview = pty.expect("? [y/N] ");
    assert!(preview.contains("  add skill notes (own)"), "{preview}");
    pty.send_line("n");
    let (code, rest) = pty.wait();
    assert_eq!(code, 1, "{rest}");
    assert!(rest.contains("not imported; nothing was changed"), "{rest}");
    assert_eq!(world(&sandbox), before);
    assert_eq!(tree(&folder), folder_before);
}

#[test]
fn a_folder_without_tools_keeps_the_repo_tools_and_one_with_tools_uses_its_own() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    write(
        repo.join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\"]\n",
    );
    let folder = import_folder(&sandbox, "[skills.notes]\nenabled = true\n");

    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);
    assert_eq!(
        read(repo.join("skiman.toml")),
        "[tools]\nenabled = [\"claude-code\"]\n[skills.notes]\nenabled = true\n"
    );
    assert!(
        sandbox
            .tool_dir(Tool::ClaudeCode)
            .join("notes")
            .is_symlink()
    );
    assert!(!sandbox.tool_dir(Tool::Codex).join("notes").exists());

    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    let folder = import_folder(&sandbox, "[tools]\nenabled = [\"codex\"]\n");
    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);
    assert_eq!(
        read(repo.join("skiman.toml")),
        "[tools]\nenabled = [\"codex\"]\n"
    );
    assert!(sandbox.tool_dir(Tool::Codex).join("notes").is_symlink());
    assert!(!sandbox.tool_dir(Tool::ClaudeCode).join("notes").exists());
}

#[test]
fn an_upstream_gitignore_in_a_local_source_is_left_out_of_the_snapshot() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let source = sandbox.home().join("src/pdf");
    write(source.join("SKILL.md"), skill_md("pdf"));
    write(source.join(".gitignore"), "*\n");
    let folder = import_folder(
        &sandbox,
        &format!("{TOOLS}\n[skills.pdf.source]\nurl = \"~/src/pdf\"\npath = \"\"\n"),
    );
    write(folder.join("skills/pdf/SKILL.md"), skill_md("pdf"));

    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);

    let snapshot = sandbox.canon().join("upstream/pdf");
    assert_eq!(read(snapshot.join("SKILL.md")), skill_md("pdf"));
    assert!(!snapshot.join(".gitignore").exists());
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

/// Registers `~/proj` as project root `proj` and adopts its `AGENTS.md`,
/// which holds a sample key exempted for `projects/proj/AGENTS.md`. Returns
/// that `[[secrets.allow]]` entry.
fn project_with_an_exempted_key(sandbox: &Sandbox) -> String {
    // Assembled at runtime so the source never holds a secret-like value.
    let key = ["AK", "IA", "Q2W3E4R5T6Y7U8I9"].concat();
    write(
        sandbox.home().join("proj/AGENTS.md"),
        format!("# Proj\n\nSample: AWS_ACCESS_KEY_ID={key}\n"),
    );
    sandbox
        .skiman()
        .args(["project", "add"])
        .arg(sandbox.home().join("proj"))
        .assert()
        .code(0);
    let entry = format!(
        "[[secrets.allow]]\npath = \"projects/proj/AGENTS.md\"\nrule = \"aws-access-key\"\nfingerprint = \"{}\"\n",
        fingerprint(&key)
    );
    let manifest = sandbox.canon().join("skiman.toml");
    std::fs::write(&manifest, format!("{}\n{entry}", read(&manifest))).unwrap();
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    entry
}

#[test]
fn import_keeps_the_repos_project_roots_and_their_secret_exemptions() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let entry = project_with_an_exempted_key(&sandbox);
    let folder = import_folder(&sandbox, TOOLS);

    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);

    assert_eq!(
        read(sandbox.canon().join("skiman.toml")),
        format!("{TOOLS}\n[projects.proj]\npath = \"~/proj\"\n\n{entry}")
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn import_adds_the_folders_project_roots_to_the_repos_and_keeps_one_copy_of_each_exemption() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let entry = project_with_an_exempted_key(&sandbox);
    write(sandbox.home().join("other/AGENTS.md"), "# Other\n");
    let folder = import_folder(
        &sandbox,
        &format!("{TOOLS}\n[projects.other]\npath = \"~/other\"\n\n{entry}"),
    );

    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);

    assert_eq!(
        read(sandbox.canon().join("skiman.toml")),
        format!(
            "{TOOLS}\n[projects.other]\npath = \"~/other\"\n\n[projects.proj]\npath = \"~/proj\"\n\n{entry}"
        )
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

/// Imports a folder whose `skiman.toml` is `folder` into a canon set up
/// by [`project_with_an_exempted_key`] whose `skiman.toml` is then `repo`
/// (given that entry). `check` passes before and after, so the root and its
/// exemption are kept, and the mirror stays. Returns the `skiman.toml` the
/// import wrote and the entry.
fn import_project_config(repo: impl FnOnce(&str) -> String, folder: &str) -> (String, String) {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let entry = project_with_an_exempted_key(&sandbox);
    let manifest = sandbox.canon().join("skiman.toml");
    std::fs::write(&manifest, repo(&entry)).unwrap();
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
    let folder = import_folder(&sandbox, folder);

    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);

    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
    assert!(sandbox.canon().join("projects/proj/AGENTS.md").is_file());
    (read(manifest), entry)
}

#[test]
fn import_keeps_the_repos_project_roots_written_as_an_inline_table() {
    let (manifest, entry) = import_project_config(
        |entry| format!("projects = {{ proj = {{ path = \"~/proj\" }} }}\n\n{TOOLS}\n{entry}"),
        TOOLS,
    );
    assert_eq!(
        manifest,
        format!("{TOOLS}\n[projects.proj]\npath = \"~/proj\"\n\n{entry}")
    );
}

#[test]
fn import_keeps_the_repos_project_root_written_as_an_inline_entry_of_projects() {
    let (manifest, entry) = import_project_config(
        |entry| format!("{TOOLS}\n[projects]\nproj = {{ path = \"~/proj\" }}\n\n{entry}"),
        TOOLS,
    );
    assert_eq!(
        manifest,
        format!("{TOOLS}\n[projects.proj]\npath = \"~/proj\"\n\n{entry}")
    );
}

#[test]
fn import_adds_the_repos_project_roots_to_an_empty_inline_projects_table_in_the_folder() {
    let (manifest, entry) = import_project_config(
        |entry| format!("{TOOLS}\n[projects.proj]\npath = \"~/proj\"\n\n{entry}"),
        &format!("projects = {{}}\n{TOOLS}"),
    );
    assert_eq!(
        manifest,
        format!("{TOOLS}\n[projects.proj]\npath = \"~/proj\"\n\n{entry}")
    );
}

#[test]
fn a_project_root_the_folder_writes_inline_wins_over_the_repos_root_with_the_same_id() {
    let projects = "projects = { proj = { path = \"~/proj\", exclude = [\"drafts\"] } }\n";
    let (manifest, entry) = import_project_config(
        |entry| format!("{TOOLS}\n[projects.proj]\npath = \"~/proj\"\n\n{entry}"),
        &format!("{projects}{TOOLS}"),
    );
    assert_eq!(manifest, format!("{projects}{TOOLS}\n{entry}"));
}

#[test]
fn import_keeps_the_repos_project_exemptions_when_the_folder_has_an_empty_inline_secrets_table() {
    let (manifest, entry) = import_project_config(
        |entry| format!("{TOOLS}\n[projects.proj]\npath = \"~/proj\"\n\n{entry}"),
        &format!("secrets = {{}}\n{TOOLS}"),
    );
    assert_eq!(
        manifest,
        format!("{TOOLS}\n[projects.proj]\npath = \"~/proj\"\n\n{entry}")
    );
}

#[test]
fn an_import_folder_with_a_skill_dirs_key_is_refused_as_retired() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let folder = import_folder(
        &sandbox,
        &format!("{TOOLS}\n[projects.proj]\npath = \"~/proj\"\nskill_dirs = [\"skills\"]\n"),
    );
    assert_refused(
        &sandbox,
        &folder,
        1,
        &[
            "skiman.toml:6: toml-value: `skill_dirs` in [projects.proj] is retired; remove it and register each folder of skills with `skiman linked add <path>`",
        ],
    );
}

#[test]
fn a_failure_during_takeover_stops_the_import_and_the_same_command_finishes_it() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write(claude.join("notes/SKILL.md"), skill_md("notes"));
    let folder = import_folder(&sandbox, TOOLS);
    let folder_before = tree(&folder);
    // The tool directory cannot be changed: moving its entry out fails.
    let read_only = std::fs::Permissions::from_mode(0o555);
    std::fs::set_permissions(&claude, read_only).unwrap();

    let output = sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(4)
        .get_output()
        .clone();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let repo = sandbox.canon();
    assert!(
        stdout.contains(&format!(
            "created {}\n",
            repo.join("skills/notes").display()
        )),
        "{stdout}"
    );
    assert!(
        stderr.contains(&format!(
            "cannot move {} to the backup",
            claude.join("notes").display()
        )),
        "{stderr}"
    );
    assert!(stderr.contains("stopped part-way"), "{stderr}");
    // No original moved; the run's backup holds only the repo files it
    // replaced, which the lines above list.
    let [first] = backup_dirs(&sandbox).try_into().unwrap();
    assert!(stderr.contains("this run moved no original"), "{stderr}");
    assert!(
        stderr.contains("run the same command again to finish it"),
        "{stderr}"
    );
    assert_eq!(read(claude.join("notes/SKILL.md")), skill_md("notes"));
    assert_eq!(tree(&folder), folder_before);

    // With the tool directory writable again, the same command finishes it.
    let output = sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains(&format!("created {}", repo.display())),
        "{stdout}"
    );
    let second = backup_dirs(&sandbox)
        .into_iter()
        .find(|dir| *dir != first)
        .expect("the second run's backup");
    assert_eq!(
        read(second.join("claude-code/notes/SKILL.md")),
        skill_md("notes")
    );
    crate::support::assert_symlink(
        claude.join("notes"),
        repo.join("skills/notes"),
        repo.join("skills/notes"),
    );
    assert_eq!(tree(&folder), folder_before);
}

/// An import folder holding own skill `notes` and third-party skill `pdf`
/// read from the local folder `url` (`path` below it).
fn local_pdf_folder(sandbox: &Sandbox, url: &str, path: &str) -> PathBuf {
    let folder = import_folder(
        sandbox,
        &format!("{TOOLS}\n[skills.pdf.source]\nurl = \"{url}\"\npath = \"{path}\"\n"),
    );
    write(folder.join("skills/pdf/SKILL.md"), skill_md("pdf"));
    folder
}

/// The import is refused before anything changes, naming skill `pdf`, its
/// source as `skiman.toml` writes it, and `entry` as the preview shows it;
/// the source at `source` is still there.
#[track_caller]
fn assert_source_refused(
    sandbox: &Sandbox,
    folder: &Path,
    written: &str,
    entry: &Path,
    source: &Path,
) {
    assert_refused(
        sandbox,
        folder,
        1,
        &[
            "in the import folder's skiman.toml, point the source at a copy",
            &format!("\n  skill pdf: source {written}, entry {}", entry.display()),
        ],
    );
    assert!(source.join("SKILL.md").is_file());
}

#[test]
fn a_local_source_the_takeover_would_move_or_reach_through_refuses_the_import_naming_it() {
    // The source is an entry to move.
    let sandbox = Sandbox::new();
    init(&sandbox);
    let source = sandbox.tool_dir(Tool::ClaudeCode).join("pdf");
    write(source.join("SKILL.md"), skill_md("pdf"));
    let folder = local_pdf_folder(&sandbox, "~/.claude/skills/pdf", "");
    assert_source_refused(&sandbox, &folder, "~/.claude/skills/pdf", &source, &source);

    // The tool directory is a link, and the source is written through it.
    let sandbox = Sandbox::new();
    init(&sandbox);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    let real = sandbox.home().join("real-skills");
    std::fs::rename(&claude, &real).unwrap();
    std::os::unix::fs::symlink(&real, &claude).unwrap();
    write(real.join("pdf/SKILL.md"), skill_md("pdf"));
    let folder = local_pdf_folder(&sandbox, "~/.claude/skills/pdf", "");
    assert_source_refused(
        &sandbox,
        &folder,
        "~/.claude/skills/pdf",
        &claude.join("pdf"),
        &real.join("pdf"),
    );

    // The source is reached through a link to move.
    let sandbox = Sandbox::new();
    init(&sandbox);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    let pack = sandbox.home().join("src/pack");
    write(pack.join("SKILL.md"), skill_md("pack"));
    write(pack.join("pdf/SKILL.md"), skill_md("pdf"));
    std::os::unix::fs::symlink(&pack, claude.join("pack")).unwrap();
    let folder = local_pdf_folder(&sandbox, "~/.claude/skills/pack/pdf", "");
    assert_source_refused(
        &sandbox,
        &folder,
        "~/.claude/skills/pack/pdf",
        &claude.join("pack"),
        &pack.join("pdf"),
    );

    // The source holds a real folder to move: the tool directory is a link
    // into it. (The link init made there is gone, as upstreams hold no links.)
    let sandbox = Sandbox::new();
    init(&sandbox);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    let stuff = sandbox.home().join("stuff");
    std::fs::create_dir_all(&stuff).unwrap();
    std::fs::rename(&claude, stuff.join("claude-skills")).unwrap();
    std::os::unix::fs::symlink(stuff.join("claude-skills"), &claude).unwrap();
    std::fs::remove_file(stuff.join("claude-skills/skiman-manual")).unwrap();
    write(stuff.join("SKILL.md"), skill_md("pdf"));
    write(
        stuff.join("claude-skills/notes/SKILL.md"),
        skill_md("notes"),
    );
    let folder = local_pdf_folder(&sandbox, "~/stuff", "");
    assert_source_refused(&sandbox, &folder, "~/stuff", &claude.join("notes"), &stuff);

    // The source is a link outside the tool directories whose target takes
    // forty `..` steps, then a link to a link to move.
    let sandbox = Sandbox::new();
    init(&sandbox);
    let home = sandbox.home();
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    let src = home.join("src");
    write(src.join("SKILL.md"), skill_md("pack"));
    write(src.join("pdf/SKILL.md"), skill_md("pdf"));
    std::fs::create_dir_all(home.join("pad")).unwrap();
    std::os::unix::fs::symlink(&src, claude.join("pdf-src")).unwrap();
    std::os::unix::fs::symlink(claude.join("pdf-src"), home.join("bridge")).unwrap();
    let target = format!("{}/{}bridge", home.display(), "pad/../".repeat(40));
    std::os::unix::fs::symlink(target, home.join("source-ref")).unwrap();
    let folder = local_pdf_folder(&sandbox, "~/source-ref", "pdf");
    assert_source_refused(
        &sandbox,
        &folder,
        "~/source-ref (pdf)",
        &claude.join("pdf-src"),
        &src.join("pdf"),
    );
}

#[test]
fn a_local_source_added_before_the_import_is_refused_when_the_canon_already_holds_the_import() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let source = sandbox.tool_dir(Tool::ClaudeCode).join("pdf");
    write(source.join("SKILL.md"), skill_md("pdf"));
    sandbox
        .skiman()
        .args(["add", "~/.claude/skills/pdf", "--yes"])
        .output()
        .unwrap();
    let repo = sandbox.canon();
    assert!(repo.join("skills/pdf/SKILL.md").is_file());
    // The import folder holds what the canon does.
    let folder = sandbox.root().join("import");
    write(
        folder.join("skills/pdf/SKILL.md"),
        read(repo.join("skills/pdf/SKILL.md")),
    );
    write(
        folder.join("instructions/shared.md"),
        read(repo.join("instructions/shared.md")),
    );
    write(folder.join("skiman.toml"), read(repo.join("skiman.toml")));

    assert_source_refused(&sandbox, &folder, "~/.claude/skills/pdf", &source, &source);
}

#[test]
fn a_local_source_outside_the_entries_that_move_is_imported_and_updated_afterwards() {
    // The tool directory holds a link to the source.
    let sandbox = Sandbox::new();
    init(&sandbox);
    let source = sandbox.home().join("src/pdf");
    write(source.join("SKILL.md"), skill_md("pdf"));
    std::os::unix::fs::symlink(&source, sandbox.tool_dir(Tool::ClaudeCode).join("pdf")).unwrap();
    let folder = local_pdf_folder(&sandbox, "~/src/pdf", "");
    let folders = vec![(sandbox, folder)];

    // The source is in the tool directory of a tool that is not enabled.
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(
        sandbox.tool_dir(Tool::Antigravity).join("pdf/SKILL.md"),
        skill_md("pdf"),
    );
    let folder = local_pdf_folder(&sandbox, "~/.gemini/config/skills/pdf", "");
    write(
        folder.join("skiman.toml"),
        read(folder.join("skiman.toml")).replace(", \"antigravity\"", ""),
    );
    let mut folders = folders;
    folders.push((sandbox, folder));

    // The source is below an entry that is not a skill, which stays.
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(
        sandbox
            .tool_dir(Tool::ClaudeCode)
            .join("vendor/skills/pdf/SKILL.md"),
        skill_md("pdf"),
    );
    let folder = local_pdf_folder(&sandbox, "~/.claude/skills/vendor", "skills/pdf");
    folders.push((sandbox, folder));

    for (sandbox, folder) in folders {
        sandbox
            .skiman()
            .arg("import")
            .arg(&folder)
            .arg("--yes")
            .assert()
            .code(0);
        sandbox.skiman().args(["update", "pdf"]).assert().code(0);
    }
}

#[test]
fn a_local_source_that_update_would_read_another_way_is_refused_before_anything_changes() {
    // A link followed by `..`, which leads elsewhere than the same path
    // read by the letter, where nothing is.
    let sandbox = Sandbox::new();
    init(&sandbox);
    let deep = sandbox.home().join("src/deep");
    write(deep.join("pdf/SKILL.md"), skill_md("pdf"));
    std::fs::create_dir_all(deep.join("a")).unwrap();
    std::fs::create_dir_all(sandbox.home().join("links")).unwrap();
    std::os::unix::fs::symlink(deep.join("a"), sandbox.home().join("links/a")).unwrap();
    let folder = local_pdf_folder(&sandbox, "~/links/a/../pdf", "");
    let missing = format!(
        "the local folder {} does not exist",
        sandbox.home().join("links/pdf").display()
    );
    assert_refused(&sandbox, &folder, 1, &[&missing]);
}

#[test]
fn a_local_source_that_is_a_link_is_read_through_it_unless_the_import_moves_the_link() {
    // A link in the tool directory, written with a trailing `/`: the import
    // would move it to the backup.
    let sandbox = Sandbox::new();
    init(&sandbox);
    let source = sandbox.home().join("src/pdf");
    write(source.join("SKILL.md"), skill_md("pdf"));
    std::os::unix::fs::symlink(&source, sandbox.tool_dir(Tool::ClaudeCode).join("pdf")).unwrap();
    let folder = local_pdf_folder(&sandbox, "~/.claude/skills/pdf/", "");
    assert_refused(
        &sandbox,
        &folder,
        1,
        &["is in, holds or is reached through an entry the import moves to the backup"],
    );

    // A link outside the tool directories, written with a trailing `/`.
    let sandbox = Sandbox::new();
    init(&sandbox);
    let source = sandbox.home().join("src/pdf");
    write(source.join("SKILL.md"), skill_md("pdf"));
    std::fs::create_dir_all(sandbox.home().join("links")).unwrap();
    std::os::unix::fs::symlink(&source, sandbox.home().join("links/pdf")).unwrap();
    let folder = local_pdf_folder(&sandbox, "~/links/pdf/", "");
    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);
    assert_eq!(
        read(sandbox.canon().join("upstream/pdf/SKILL.md")),
        skill_md("pdf")
    );
    sandbox.skiman().args(["update", "pdf"]).assert().code(0);
}

#[test]
fn a_local_source_is_read_by_the_letter_past_a_link_and_dot_dots_as_update_reads_it() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let home = sandbox.home();
    write(home.join("src/deep/pdf/SKILL.md"), skill_md("pdf"));
    write(home.join("src/deep/pdf/reference.md"), "# Deep\n");
    std::fs::create_dir_all(home.join("src/deep/a")).unwrap();
    write(home.join("links/pdf/SKILL.md"), skill_md("pdf"));
    write(home.join("links/pdf/reference.md"), "# By the letter\n");
    std::os::unix::fs::symlink(home.join("src/deep/a"), home.join("links/a")).unwrap();
    let folder = local_pdf_folder(&sandbox, "~/links/a/../pdf", "");
    write(folder.join("skills/pdf/reference.md"), "# By the letter\n");

    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);
    assert_eq!(
        read(sandbox.canon().join("upstream/pdf/reference.md")),
        "# By the letter\n"
    );
    sandbox.skiman().args(["update", "pdf"]).assert().code(0);
}

/// Foreign skills to move whichever way the import goes: `later` and
/// `spare` (which holds a folder `pdf`) in Claude Code's tool directory, and
/// in Codex's a link to `later`.
fn more_to_move(sandbox: &Sandbox) {
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write(claude.join("later/SKILL.md"), skill_md("later"));
    write(claude.join("spare/SKILL.md"), skill_md("spare"));
    write(claude.join("spare/pdf/SKILL.md"), skill_md("pdf"));
    std::os::unix::fs::symlink(
        claude.join("later"),
        sandbox.tool_dir(Tool::Codex).join("later"),
    )
    .unwrap();
}

/// [`setup`] killed before its last skill, so the import writes again, and
/// [`more_to_move`].
fn stopped_before_the_last_skill(sandbox: &Sandbox, local: bool) -> PathBuf {
    let folder = setup_with_source(sandbox, local);
    import_killed_at(sandbox, &folder, "write:skills/pdf");
    more_to_move(sandbox);
    folder
}

/// [`setup`] imported, so the canon holds it, and [`more_to_move`].
fn imported(sandbox: &Sandbox, local: bool) -> PathBuf {
    let folder = setup_with_source(sandbox, local);
    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);
    more_to_move(sandbox);
    folder
}

/// [`setup`]; with `local`, `pdf` is read from `pdf` below the link
/// `~/src-link -> ~/src` instead.
fn setup_with_source(sandbox: &Sandbox, local: bool) -> PathBuf {
    let Setup { folder, .. } = setup(sandbox);
    if local {
        let src = sandbox.home().join("src");
        write(src.join("pdf/SKILL.md"), skill_md("pdf"));
        write(src.join("pdf/reference.md"), "# Reference\n");
        std::os::unix::fs::symlink(&src, sandbox.home().join("src-link")).unwrap();
        write(
            folder.join("skiman.toml"),
            format!("{TOOLS}\n[skills.pdf.source]\nurl = \"~/src-link\"\npath = \"pdf\"\n"),
        );
    }
    folder
}

/// Runs the import with `run` at the step between the confirmation and the
/// canon lock: it must refuse (exit 1) before anything is written or moved,
/// making no backup folder after `run`, and no backup folder may hold a link
/// into the canon.
#[track_caller]
fn assert_refused_after_the_confirmation(sandbox: &Sandbox, folder: &Path, run: &str) {
    let listed = sandbox.root().join("backups-after-the-hook");
    let output = import_with_hook(
        sandbox,
        folder,
        "import:confirmed",
        &format!(
            "{run} && (ls -1 '{}' > '{}' 2> /dev/null; true)",
            crate::support::backups(sandbox).display(),
            listed.display()
        ),
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{run}: {stdout}\n{stderr}");
    assert!(stderr.contains("nothing was changed"), "{run}: {stderr}");
    assert!(
        !stdout.lines().any(|l| ["moved ", "created ", "saved "]
            .iter()
            .any(|w| l.starts_with(w))),
        "{run}: {stdout}"
    );
    let names: Vec<String> = backup_dirs(sandbox)
        .iter()
        .map(|dir| dir.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        read(&listed).lines().collect::<Vec<_>>(),
        "{run}: the refused run made a backup folder"
    );
    let repo = sandbox.canon();
    for dir in backup_dirs(sandbox) {
        for (path, value) in tree(&dir) {
            assert!(
                !value.starts_with(&format!("link -> {}", repo.display())),
                "{run}: {} links into the canon",
                dir.join(path).display()
            );
        }
    }
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_change_between_the_confirmation_and_the_canon_lock_refuses_the_import_before_anything_moves() {
    for start in [stopped_before_the_last_skill, imported] {
        let changes: [fn(&Sandbox, &Path) -> String; 7] = [
            |sandbox, _| {
                let notes = sandbox.canon().join("skills/notes/SKILL.md");
                format!("echo edited >> '{}'", notes.display())
            },
            |sandbox, _| {
                let manifest = sandbox.canon().join("skiman.toml");
                format!("echo '# edited' >> '{}'", manifest.display())
            },
            |_, folder| {
                let notes = folder.join("skills/notes/SKILL.md");
                format!("echo edited >> '{}'", notes.display())
            },
            |_, folder| {
                format!(
                    "env -u SKIMAN_TEST_HOOK '{}' import '{}' --yes > /dev/null 2>&1",
                    env!("CARGO_BIN_EXE_skiman"),
                    folder.display()
                )
            },
            |sandbox, _| {
                let plugin = sandbox
                    .tool_dir(Tool::ClaudeCode)
                    .join("later/.claude-plugin");
                format!(
                    "mkdir '{}' && echo '{{}}' > '{}/plugin.json'",
                    plugin.display(),
                    plugin.display()
                )
            },
            |sandbox, _| {
                let link = sandbox.tool_dir(Tool::Codex).join("later");
                let spare = sandbox.tool_dir(Tool::ClaudeCode).join("spare");
                format!("ln -sfn '{}' '{}'", spare.display(), link.display())
            },
            |sandbox, _| {
                let fifo = sandbox.tool_dir(Tool::ClaudeCode).join("later/fifo");
                format!("mkfifo '{}'", fifo.display())
            },
        ];
        for change in changes {
            let sandbox = Sandbox::new();
            let folder = start(&sandbox, false);
            assert_refused_after_the_confirmation(&sandbox, &folder, &change(&sandbox, &folder));
        }
    }

    // Where the canon holds the import, a skiman.toml that changes no
    // condition is still checked byte for byte: Codex is no longer enabled,
    // and its entry stays where it is.
    let sandbox = Sandbox::new();
    let folder = imported(&sandbox, false);
    let manifest = sandbox.canon().join("skiman.toml");
    let without_codex = read(&manifest).replace(", \"codex\"", "");
    std::fs::write(sandbox.root().join("without-codex.toml"), without_codex).unwrap();
    assert_refused_after_the_confirmation(
        &sandbox,
        &folder,
        &format!(
            "cp '{}' '{}'",
            sandbox.root().join("without-codex.toml").display(),
            manifest.display()
        ),
    );
    assert!(sandbox.tool_dir(Tool::Codex).join("later").is_symlink());
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_canon_file_changed_while_the_import_fetches_refuses_the_import_and_keeps_the_change() {
    // Which way the import goes, and the skiman.toml it writes, follow the
    // canon's three files as first read. The preview is worked out after
    // the fetch, so it already holds the change: only the bytes as first read
    // tell it apart.
    let changes = [
        (
            "skiman.toml",
            r#"env -u SKIMAN_TEST_HOOK "$SKIMAN" project add "$PROJECT" > /dev/null"#,
        ),
        (
            "instructions/shared.md",
            r#"echo 'Written in a session.' >> "$FILE""#,
        ),
        ("skiman.lock", r#"echo '# written' >> "$FILE""#),
    ];
    for (rel, change) in changes {
        let sandbox = Sandbox::new();
        let folder = setup_with_source(&sandbox, false);
        let project = sandbox.home().join("proj");
        write(project.join("AGENTS.md"), "# Proj\n");
        let file = sandbox.canon().join(rel);
        let changed = sandbox.root().join("changed");
        let run = format!(
            "FILE='{}' SKIMAN='{}' PROJECT='{}'; {change} && cp \"$FILE\" '{}'",
            file.display(),
            env!("CARGO_BIN_EXE_skiman"),
            project.display(),
            changed.display()
        );
        let output = import_with_hook(&sandbox, &folder, "import:fetched", &run);
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            "error: the canon changed after the preview; nothing was changed, run the import again\n",
            "{rel}"
        );
        assert_eq!(output.status.code(), Some(1), "{rel}");
        assert_eq!(read(&file), read(&changed), "{rel}");
        assert!(backup_dirs(&sandbox).is_empty(), "{rel}");
    }
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_local_source_pointed_at_an_entry_to_move_after_the_confirmation_refuses_the_import() {
    for start in [stopped_before_the_last_skill, imported] {
        let sandbox = Sandbox::new();
        let folder = start(&sandbox, true);
        let spare = sandbox.tool_dir(Tool::ClaudeCode).join("spare");
        let link = sandbox.home().join("src-link");
        let run = format!("ln -sfn '{}' '{}'", spare.display(), link.display());
        assert_refused_after_the_confirmation(&sandbox, &folder, &run);
        assert!(spare.join("SKILL.md").is_file());
    }
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_session_start_sync_or_an_edit_inside_an_entry_after_the_confirmation_does_not_refuse_the_import()
 {
    // Codex's instruction file is a link to Claude Code's, which is gone
    // after the kill; a session start in between links it again.
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    let agents_md = sandbox.instruction_file(Tool::Codex);
    std::fs::remove_file(&agents_md).unwrap();
    std::os::unix::fs::symlink(sandbox.instruction_file(Tool::ClaudeCode), &agents_md).unwrap();
    import_killed_at(&sandbox, &folder, "moved:claude-code/CLAUDE.md");
    let output = import_with_hook(
        &sandbox,
        &folder,
        "import:confirmed",
        &format!("'{}' sync --hook", env!("CARGO_BIN_EXE_skiman")),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let sandbox = Sandbox::new();
    let folder = imported(&sandbox, false);
    let later = sandbox.tool_dir(Tool::ClaudeCode).join("later/SKILL.md");
    let output = import_with_hook(
        &sandbox,
        &folder,
        "import:confirmed",
        &format!("echo edited >> '{}'", later.display()),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_default_import_folder_inside_the_canon_works_and_is_left_in_place() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    let folder = repo.join(".skiman/import");
    write(folder.join("skills/notes/SKILL.md"), skill_md("notes"));
    write(folder.join("instructions/shared.md"), "Shared.\n");
    write(folder.join("skiman.toml"), TOOLS);
    let folder_before = tree(&folder);

    sandbox
        .skiman()
        .current_dir(&repo)
        .args(["import", ".skiman/import", "--yes"])
        .assert()
        .code(0);
    assert_eq!(tree(&folder), folder_before);
    assert_eq!(read(repo.join("skills/notes/SKILL.md")), skill_md("notes"));
    sandbox.skiman().arg("check").assert().code(0);
}

/// Spec Import: the import folder defaults to `<canon>/.skiman/import/`.
#[test]
fn import_without_a_folder_uses_the_default_import_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    let folder = repo.join(".skiman/import");
    write(folder.join("skills/notes/SKILL.md"), skill_md("notes"));
    write(folder.join("instructions/shared.md"), "Shared.\n");
    write(folder.join("skiman.toml"), TOOLS);

    sandbox.skiman().args(["import", "--yes"]).assert().code(0);
    assert_eq!(read(repo.join("skills/notes/SKILL.md")), skill_md("notes"));
    assert_eq!(read(repo.join("instructions/shared.md")), "Shared.\n");
}
