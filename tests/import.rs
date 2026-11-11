//! `skiman import <folder>`: bring an existing setup in from a prepared
//! import folder (spec Import, ADR 0001).

mod support;

use std::collections::BTreeMap;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};

use support::{Sandbox, Tool, tree};

fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: The {name} skill\n---\n\n# {name}\n")
}

/// An import folder outside the skill repo holding own skill `notes`, a
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

/// The whole skill repo, every tool directory and instruction file, and the
/// machine state except the git cache (fetching may fill it), to prove a
/// command changed nothing.
fn world(sandbox: &Sandbox) -> Vec<BTreeMap<PathBuf, String>> {
    let mut state = tree(sandbox.state_home());
    state.retain(|path, _| path != Path::new("skiman") && !path.starts_with("skiman/cache"));
    let mut world = vec![tree(sandbox.skill_repo()), state];
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

/// Hex sha256 of `bytes`, prefixed like the import record's file hashes.
fn file_hash(bytes: impl AsRef<[u8]>) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes.as_ref());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

/// A messy setup: instruction files that are regular files, a skill copied
/// into one tool directory and linked from another, a copy the import
/// leaves out, a managed-elsewhere folder, a non-skill file at a skill's
/// name, and an upstream `acme/skills` holding `pdf`.
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
        .write("skills/pdf/reference.md", "# Reference\n");
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
        .arg(sandbox.skill_repo())
        .assert()
        .code(3);
    let repo = sandbox.skill_repo();
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
        "import {folder} into the skill repo {repo}:
  add skill notes (own)
  add skill pdf (third-party, from https://github.com/acme/skills.git (skills/pdf) at {commit} (commit)); files that differ from upstream:
      modified SKILL.md
  move {repo}/instructions/shared.md to the backup, then write the import's
  move {repo}/skiman.toml to the backup, then write the import's
  move {claude}/notes to the backup, then link it to skill notes
  move {claude}/old-copy to the backup (no imported skill of that name is distributed)
  move {codex}/notes to the backup, then link it to skill notes
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
                .and_then(|l| l.split_once("; the originals are in "))
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

    // The skill repo holds the import.
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
        support::assert_symlink(
            dir.join("notes"),
            repo.join("skills/notes"),
            repo.join("skills/notes"),
        );
        support::assert_symlink(
            dir.join("pdf"),
            repo.join("skills/pdf"),
            repo.join("skills/pdf"),
        );
    }
    assert!(std::fs::symlink_metadata(claude.join("old-copy")).is_err());
    for tool in [Tool::ClaudeCode, Tool::Codex, Tool::Antigravity] {
        let shared = repo.join("instructions/shared.md");
        support::assert_symlink(sandbox.instruction_file(tool), &shared, &shared);
    }
    // Left alone: managed elsewhere, and the non-skill entry.
    assert_eq!(tree(claude.join("synced")), synced_before);
    assert_eq!(read(gemini.join("notes")), "not a skill\n");
    assert_eq!(tree(&folder), folder_before);

    // Machine state holds the finished import's record.
    let record: toml_edit::DocumentMut =
        read(sandbox.state_home().join("skiman/import/record.toml"))
            .parse()
            .unwrap();
    assert_eq!(record["status"].as_str(), Some("finished"));
    assert_eq!(record["folder"].as_str(), Some(folder.to_str().unwrap()));
    assert_eq!(record["backup"].as_str(), Some(backup.to_str().unwrap()));
    assert_eq!(
        record["pre_import"]["shared_body"].as_str(),
        Some(file_hash("old shared body\n").as_str())
    );
    assert_eq!(
        record["pre_import"]["manifest"].as_str(),
        Some(file_hash(&old_manifest).as_str())
    );
    assert_eq!(
        record["candidate"]["manifest"].as_str(),
        Some(file_hash(&manifest).as_str())
    );
    assert_eq!(
        record["candidate"]["lock"].as_str(),
        Some(file_hash(&lock).as_str())
    );
    assert_eq!(record["lock_content"].as_str(), Some(lock.as_str()));
    let locations = record["locations"].as_array_of_tables().unwrap();
    assert_eq!(locations.len(), 5);
    assert!(
        locations
            .iter()
            .all(|l| l["state"].as_str() == Some("taken-over"))
    );
    assert!(!sandbox.state_home().join("skiman/import/area").exists());
}

/// Every case leaves everything unchanged: the import folder, the skill
/// repo, the tool directories, the instruction files and machine state.
#[track_caller]
fn assert_refused(sandbox: &Sandbox, folder: &Path, code: i32, stderr_has: &[&str]) {
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
}

#[test]
fn a_skill_repo_with_a_skill_a_snapshot_or_a_lock_entry_refuses_import() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    let folder = import_folder(&sandbox, TOOLS);
    write(repo.join("skills/.hidden/x"), "dot names do not count\n");
    write(repo.join("upstream/.keep"), "");

    write(repo.join("skills/mine/SKILL.md"), skill_md("mine"));
    assert_refused(&sandbox, &folder, 1, &["already holds skills/mine"]);
    std::fs::remove_dir_all(repo.join("skills/mine")).unwrap();

    write(repo.join("upstream/pdf/SKILL.md"), skill_md("pdf"));
    assert_refused(&sandbox, &folder, 1, &["already holds upstream/pdf"]);
    std::fs::remove_dir_all(repo.join("upstream/pdf")).unwrap();

    write(
        repo.join("skiman.lock"),
        "version = 1\n\n[skills.pdf]\nsnapshot = \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"\n",
    );
    assert_refused(
        &sandbox,
        &folder,
        1,
        &["already holds a skiman.lock entry for pdf"],
    );
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
        read(sandbox.skill_repo().join("upstream/pdf/tests/fixture.env")),
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
    let repo_before = tree(sandbox.skill_repo());
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
    assert_eq!(tree(sandbox.skill_repo()), repo_before);
    assert_eq!(tree(&folder), folder_before);
    assert!(!sandbox.state_home().join("skiman/import").exists());
    assert!(!sandbox.state_home().join("skiman/backups").exists());
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
    let repo = sandbox.skill_repo();
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
    let repo = sandbox.skill_repo();
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

    let snapshot = sandbox.skill_repo().join("upstream/pdf");
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
        file_hash(&key)
    );
    let manifest = sandbox.skill_repo().join("skiman.toml");
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
        read(sandbox.skill_repo().join("skiman.toml")),
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
        read(sandbox.skill_repo().join("skiman.toml")),
        format!(
            "{TOOLS}\n[projects.other]\npath = \"~/other\"\n\n[projects.proj]\npath = \"~/proj\"\n\n{entry}"
        )
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

/// The takeover judges entries by the `skiman.toml` the import writes, not
/// the one it replaces.
#[test]
fn a_link_into_skill_dirs_that_only_the_import_folder_declares_is_left_in_place() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let target = sandbox.home().join("proj/skills/foo");
    write(target.join("SKILL.md"), skill_md("foo"));
    let link = sandbox.tool_dir(Tool::ClaudeCode).join("foo");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let folder = import_folder(
        &sandbox,
        &format!("{TOOLS}\n[projects.proj]\npath = \"~/proj\"\nskill_dirs = [\"skills\"]\n"),
    );

    sandbox
        .skiman()
        .arg("import")
        .arg(&folder)
        .arg("--yes")
        .assert()
        .code(0);
    assert_eq!(std::fs::read_link(&link).unwrap(), target);
}

#[test]
fn a_failure_during_takeover_stops_the_import_keeps_the_record_and_the_same_command_resumes_it() {
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
    let repo = sandbox.skill_repo();
    assert!(
        stdout.contains(&format!(
            "created {}\n",
            repo.join("skills/notes").display()
        )),
        "{stdout}"
    );
    let record_file = sandbox.state_home().join("skiman/import/record.toml");
    assert!(
        stderr.contains(&format!(
            "cannot move {} to the backup",
            claude.join("notes").display()
        )),
        "{stderr}"
    );
    assert!(stderr.contains("the import stopped part-way"), "{stderr}");
    assert!(
        stderr.contains(&record_file.display().to_string()),
        "{stderr}"
    );
    assert_eq!(read(claude.join("notes/SKILL.md")), skill_md("notes"));
    assert_eq!(tree(&folder), folder_before);

    let record: toml_edit::DocumentMut = read(&record_file).parse().unwrap();
    assert_eq!(record["status"].as_str(), Some("in-progress"));
    let backup = PathBuf::from(record["backup"].as_str().unwrap());
    assert!(stderr.contains(&backup.display().to_string()), "{stderr}");
    assert_eq!(
        record["skills"]["notes"]["skill"].as_str().map(str::len),
        Some("sha256:".len() + 64)
    );
    let location = &record["locations"]
        .as_array_of_tables()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(
        location["path"].as_str(),
        Some(claude.join("notes").to_str().unwrap())
    );
    assert_eq!(location["state"].as_str(), Some("planned"));
    assert!(
        sandbox
            .state_home()
            .join("skiman/import/area/skiman.lock")
            .is_file()
    );

    assert!(
        stderr.contains("run the same command again with the import folder unchanged"),
        "{stderr}"
    );

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
    assert!(stdout.contains("resuming the import of"), "{stdout}");
    assert!(
        !stdout.contains(&format!("created {}", repo.display())),
        "{stdout}"
    );
    assert_eq!(
        read(backup.join("claude-code/notes/SKILL.md")),
        skill_md("notes")
    );
    support::assert_symlink(
        claude.join("notes"),
        repo.join("skills/notes"),
        repo.join("skills/notes"),
    );
    let record: toml_edit::DocumentMut = read(&record_file).parse().unwrap();
    assert_eq!(record["status"].as_str(), Some("finished"));
    assert!(!sandbox.state_home().join("skiman/import/area").exists());
    assert_eq!(tree(&folder), folder_before);
}

#[test]
fn the_default_import_folder_inside_the_skill_repo_works_and_is_left_in_place() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
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

/// Spec Import: the import folder defaults to `<skill repo>/.skiman/import/`.
#[test]
fn import_without_a_folder_uses_the_default_import_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    let folder = repo.join(".skiman/import");
    write(folder.join("skills/notes/SKILL.md"), skill_md("notes"));
    write(folder.join("instructions/shared.md"), "Shared.\n");
    write(folder.join("skiman.toml"), TOOLS);

    sandbox.skiman().args(["import", "--yes"]).assert().code(0);
    assert_eq!(read(repo.join("skills/notes/SKILL.md")), skill_md("notes"));
    assert_eq!(read(repo.join("instructions/shared.md")), "Shared.\n");
}
