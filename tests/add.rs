//! `skiman add owner/repo`: install a single third-party skill from a GitHub
//! upstream (reached through the sandbox's `insteadOf` rule, no network).

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use snapbox::str;
use support::{Sandbox, Tool, Upstream, assert_symlink, tree};

fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

fn is_executable(path: impl AsRef<Path>) -> bool {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o100 != 0
}

/// An upstream `acme/<repo>` whose root is one skill.
fn single_skill(sandbox: &Sandbox, repo: &str, skill_md: &str) -> Upstream {
    let up = sandbox.upstream(&format!("acme/{repo}"));
    up.write("SKILL.md", skill_md)
        .write_executable("scripts/fill.sh", "#!/bin/sh\necho fill\n")
        .write("reference.md", "# Reference\n");
    up.commit("v1");
    up
}

const PDF_SKILL_MD: &str = "---\nname: pdf\ndescription: Fill PDF forms\n---\n\n# pdf\n";

/// The whole skill repo (including `.git` and `.skiman`) and every tool
/// directory, to prove a command changed nothing.
fn world(sandbox: &Sandbox) -> Vec<std::collections::BTreeMap<PathBuf, String>> {
    let mut world = vec![tree(sandbox.skill_repo())];
    world.extend(Tool::ALL.iter().map(|t| tree(sandbox.tool_dir(*t))));
    world
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The spec's tree hash, computed here independently of skiman: regular
/// files only, sorted by path bytes, `path \0 x \0 sha256 \n`.
fn expected_tree_hash(dir: &Path) -> String {
    fn files(root: &Path, dir: &Path, out: &mut Vec<(String, bool, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                files(root, &path, out);
            } else if meta.is_file() {
                let rel = path.strip_prefix(root).unwrap().to_str().unwrap();
                let exec = meta.permissions().mode() & 0o100 != 0;
                out.push((rel.to_owned(), exec, std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut list = Vec::new();
    files(dir, dir, &mut list);
    list.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut text = Vec::new();
    for (rel, exec, content) in list {
        text.extend_from_slice(rel.as_bytes());
        text.extend_from_slice(if exec { b"\x001\x00" } else { b"\x000\x00" });
        text.extend_from_slice(hex(&Sha256::digest(&content)).as_bytes());
        text.push(b'\n');
    }
    format!("sha256:{}", hex(&Sha256::digest(&text)))
}

fn has_commit(sandbox: &Sandbox) -> bool {
    sandbox
        .std_command("git")
        .current_dir(sandbox.skill_repo())
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .status()
        .unwrap()
        .success()
}

#[test]
fn add_installs_a_single_skill_records_it_and_links_it() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    let repo = sandbox.skill_repo();

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skill pdf from https://github.com/acme/pdf.git at [..] (default branch main):
  SKILL.md
  reference.md
  scripts/fill.sh  [script, executable]
created [..]/skill-repo/upstream/pdf
created [..]/skill-repo/skills/pdf
changed [..]/skill-repo/skiman.toml
created [..]/skill-repo/skiman.lock
linked [..]/home/.claude/skills/pdf
linked [..]/home/.agents/skills/pdf
linked [..]/home/.gemini/config/skills/pdf

"#]])
        .stderr_eq("");

    for root in ["skills/pdf", "upstream/pdf"] {
        let dir = repo.join(root);
        assert_eq!(read(dir.join("SKILL.md")), PDF_SKILL_MD);
        assert_eq!(read(dir.join("reference.md")), "# Reference\n");
        assert!(is_executable(dir.join("scripts/fill.sh")));
        assert!(!is_executable(dir.join("SKILL.md")));
    }
    assert_eq!(
        read(repo.join("skiman.toml")),
        "[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n\n\
         [skills.pdf.source]\nurl = \"https://github.com/acme/pdf.git\"\npath = \"\"\n"
    );
    assert_eq!(
        read(repo.join("skiman.lock")),
        format!(
            "version = 1\n\n[skills.pdf]\ncommit = \"{}\"\nsnapshot = \"{}\"\n",
            up.head(),
            expected_tree_hash(&repo.join("upstream/pdf"))
        )
    );
    let target = repo.join("skills/pdf");
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join("pdf"), &target, &target);
    }
    assert!(!has_commit(&sandbox), "add must not commit");
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn a_hand_edited_skiman_toml_keeps_every_byte_outside_the_new_table() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes"])
        .assert()
        .code(0);
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    let manifest = sandbox.skill_repo().join("skiman.toml");
    let hand_edited = "# My skills\n[tools]\nenabled = [ \"claude-code\",\"codex\" ]   # trimmed by hand\n\n\
        [skills.notes]   # parked\nenabled = false\n\n\
        [[secrets.allow]]\npath = 'skills/notes/x.env'\nrule = \"aws-access-key\"\nfingerprint = \"sha256:00\"\n# trailing comment\n";
    std::fs::write(&manifest, hand_edited).unwrap();

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);

    let text = read(&manifest);
    let table = "\n[skills.pdf.source]\nurl = \"https://github.com/acme/pdf.git\"\npath = \"\"\n";
    let at = text
        .find(table)
        .unwrap_or_else(|| panic!("no new table in:\n{text}"));
    assert_eq!(
        format!("{}{}", &text[..at], &text[at + table.len()..]),
        hand_edited,
        "everything outside the new table is unchanged:\n{text}"
    );
}

#[test]
fn dry_run_prints_the_file_list_and_changes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--dry-run"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skill pdf from https://github.com/acme/pdf.git at [..] (default branch main):
  SKILL.md
  reference.md
  scripts/fill.sh  [script, executable]
dry run: nothing was written

"#]])
        .stderr_eq("");

    assert_eq!(world(&sandbox), before);
}

#[test]
fn without_a_terminal_add_needs_yes_or_dry_run_and_writes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/pdf"])
        .assert()
        .code(2)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: missing flag without a terminal: --yes (install) or --dry-run (only show the file list)

"#]]);

    assert_eq!(world(&sandbox), before);
}

#[test]
fn with_a_terminal_add_asks_and_declining_writes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    let before = world(&sandbox);

    let mut pty = sandbox.skiman_pty(["add", "acme/pdf"]);
    let shown = pty.expect("Install skill pdf? [y/N] ");
    assert!(
        shown.contains("scripts/fill.sh  [script, executable]"),
        "{shown}"
    );
    pty.send_line("n");
    let (code, rest) = pty.wait();
    assert_eq!(code, 1, "{rest}");
    assert!(
        rest.contains("not installed; nothing was written"),
        "{rest}"
    );

    assert_eq!(world(&sandbox), before);
}

#[test]
fn with_a_terminal_add_installs_after_yes() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);

    let mut pty = sandbox.skiman_pty(["add", "acme/pdf"]);
    pty.expect("Install skill pdf? [y/N] ");
    pty.send_line("y");
    let (code, rest) = pty.wait();
    assert_eq!(code, 0, "{rest}");

    let target = sandbox.skill_repo().join("skills/pdf");
    assert_symlink(sandbox.tool_dir(Tool::Codex).join("pdf"), &target, &target);
}

#[test]
fn a_taken_name_is_refused_and_the_message_names_the_name_flag() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "My own pdf skill"])
        .assert()
        .code(0);
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: the name `pdf` is taken: skills/pdf already exists; install it under another name with `--name <name>`

"#]]);

    assert_eq!(world(&sandbox), before);
}

#[test]
fn name_installs_under_another_name_rewriting_the_name_line_only_in_the_skill() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skill_md = "---\ndescription: Fill PDF forms\nname: pdf   # upstream's own name\nlicense: MIT\n---\n\n# pdf\n";
    single_skill(&sandbox, "pdf", skill_md);
    let repo = sandbox.skill_repo();

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--name", "pdf-forms", "--yes"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skill pdf-forms (upstream name pdf) from https://github.com/acme/pdf.git at [..] (default branch main):
  SKILL.md
  reference.md
  scripts/fill.sh  [script, executable]
the `name` line of skills/pdf-forms/SKILL.md is set to `pdf-forms` (a local modification; upstream/pdf-forms keeps the upstream text)
created [..]/skill-repo/upstream/pdf-forms
created [..]/skill-repo/skills/pdf-forms
...
"#]])
        .stderr_eq("");

    assert_eq!(
        read(repo.join("skills/pdf-forms/SKILL.md")),
        "---\ndescription: Fill PDF forms\nname: pdf-forms\nlicense: MIT\n---\n\n# pdf\n"
    );
    assert_eq!(read(repo.join("upstream/pdf-forms/SKILL.md")), skill_md);
    assert!(
        read(repo.join("skiman.toml")).ends_with(
            "[skills.pdf-forms.source]\nurl = \"https://github.com/acme/pdf.git\"\npath = \"\"\nupstream_name = \"pdf\"\n"
        ),
        "{}",
        read(repo.join("skiman.toml"))
    );
    let target = repo.join("skills/pdf-forms");
    assert_symlink(
        sandbox.tool_dir(Tool::ClaudeCode).join("pdf-forms"),
        &target,
        &target,
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn an_upstream_without_a_name_line_gets_one_inserted_and_is_named_after_the_repo() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skill_md = "---\ndescription: Take notes\n---\n\n# Notes\n";
    single_skill(&sandbox, "notes", skill_md);
    let repo = sandbox.skill_repo();

    sandbox
        .skiman()
        .args(["add", "acme/notes", "--yes"])
        .assert()
        .code(0);

    assert_eq!(
        read(repo.join("skills/notes/SKILL.md")),
        "---\nname: notes\ndescription: Take notes\n---\n\n# Notes\n"
    );
    assert_eq!(read(repo.join("upstream/notes/SKILL.md")), skill_md);
    assert!(
        !read(repo.join("skiman.toml")).contains("upstream_name"),
        "the upstream name equals the install name"
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn a_multi_line_name_is_refused_with_an_explanation() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    single_skill(
        &sandbox,
        "pdf",
        "---\nname: pdf\n  forms\ndescription: Fill PDF forms\n---\n",
    );
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--name", "pdf", "--yes"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: SKILL.md in https://github.com/acme/pdf.git has a `name` entry that is not a single-line `name: <value>` line; skiman only rewrites a single-line `name:` entry, so it cannot set the name to `pdf`

"#]]);

    assert_eq!(world(&sandbox), before);
}

#[test]
fn missing_frontmatter_and_missing_description_are_refused_naming_the_file() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    single_skill(&sandbox, "bare", "# Just a body\n");
    single_skill(&sandbox, "nodesc", "---\nname: nodesc\n---\n");
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/bare", "--yes"])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
error: SKILL.md in https://github.com/acme/bare.git has no frontmatter: it must start on line 1 with `---` and end with a `---` line

"#]]);
    sandbox
        .skiman()
        .args(["add", "acme/nodesc", "--yes"])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
error: SKILL.md in https://github.com/acme/nodesc.git has no `description` in its frontmatter

"#]]);

    assert_eq!(world(&sandbox), before);
}

/// Plants a tmp leftover: a folder in `.skiman/tmp/` that no running command staged.
fn plant_tmp_leftover(sandbox: &Sandbox) -> PathBuf {
    let leftover = sandbox
        .skill_repo()
        .join(".skiman/tmp/20250101T000000Z/old/skills/pdf");
    std::fs::create_dir_all(&leftover).unwrap();
    std::fs::write(leftover.join("SKILL.md"), "an edit worth keeping\n").unwrap();
    sandbox.skill_repo().join(".skiman/tmp/20250101T000000Z")
}

fn backups(sandbox: &Sandbox) -> PathBuf {
    sandbox.state_home().join("skiman/backups")
}

#[test]
fn add_moves_tmp_leftovers_to_a_backup_and_prints_the_path() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    let leftover = plant_tmp_leftover(&sandbox);

    let output = sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0)
        .stderr_eq("")
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();

    assert!(!leftover.exists(), "the tmp leftover left .skiman/tmp/");
    let backup_dirs: Vec<PathBuf> = std::fs::read_dir(backups(&sandbox))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(backup_dirs.len(), 1, "{backup_dirs:?}");
    let moved_to = backup_dirs[0].join("repo/.skiman/tmp");
    assert_eq!(
        read(moved_to.join("20250101T000000Z/old/skills/pdf/SKILL.md")),
        "an edit worth keeping\n"
    );
    assert!(
        stdout.contains(&format!("moved tmp leftovers to {}\n", moved_to.display())),
        "{stdout}"
    );
    let name = backup_dirs[0].file_name().unwrap().to_str().unwrap();
    assert!(is_backup_timestamp(name), "{name}");
}

/// UTC `YYYYMMDDTHHMMSSZ`.
fn is_backup_timestamp(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 16
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'T'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'Z'
}

#[test]
fn a_refused_add_leaves_tmp_leftovers_where_they_are() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "Mine"])
        .assert()
        .code(0);
    let leftover = plant_tmp_leftover(&sandbox);

    sandbox.skiman().args(["add", "acme/pdf"]).assert().code(2);
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(1);

    assert!(leftover.join("old/skills/pdf/SKILL.md").is_file());
    assert!(!backups(&sandbox).exists());
}

#[test]
fn an_unwritable_backups_folder_stops_add_with_exit_4_and_keeps_the_tmp_leftover() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    let leftover = plant_tmp_leftover(&sandbox);
    let backups = backups(&sandbox);
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o555)).unwrap();

    let assert = sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(4);
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o755)).unwrap();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();

    let tmp = sandbox.skill_repo().join(".skiman/tmp");
    assert!(
        stderr.contains(&format!(
            "move {} out of the skill repo by hand",
            tmp.display()
        )),
        "{stderr}"
    );
    assert!(leftover.join("old/skills/pdf/SKILL.md").is_file());
    assert!(!sandbox.skill_repo().join("skills/pdf").exists());
    assert!(!sandbox.skill_repo().join("upstream/pdf").exists());
}

/// `date -u` for `secs` since the epoch, as `YYYYMMDDTHHMMSSZ`.
fn utc_stamp(secs: u64) -> String {
    let out = std::process::Command::new("date")
        .args(["-u", "-r", &secs.to_string(), "+%Y%m%dT%H%M%SZ"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn a_backup_folder_whose_timestamp_exists_gets_the_2_suffix() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    plant_tmp_leftover(&sandbox);
    // Every timestamp the command can use in the next minute is taken.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let taken: Vec<String> = (now..now + 60).map(utc_stamp).collect();
    for stamp in &taken {
        std::fs::create_dir_all(backups(&sandbox).join(stamp)).unwrap();
    }

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);

    let suffixed: Vec<String> = std::fs::read_dir(backups(&sandbox))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|name| !taken.contains(name))
        .collect();
    assert_eq!(suffixed.len(), 1, "{suffixed:?}");
    let (stamp, suffix) = suffixed[0].split_at(16);
    assert!(taken.iter().any(|t| t == stamp), "{}", suffixed[0]);
    assert_eq!(suffix, "-2");
    assert!(
        backups(&sandbox)
            .join(&suffixed[0])
            .join("repo/.skiman/tmp/20250101T000000Z/old/skills/pdf/SKILL.md")
            .is_file()
    );
    for stamp in &taken {
        assert_eq!(
            std::fs::read_dir(backups(&sandbox).join(stamp))
                .unwrap()
                .count(),
            0,
            "an existing backup folder is never reused"
        );
    }
}

#[test]
fn upstream_content_that_breaks_a_rule_is_refused_and_nothing_is_written() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    up.write("notes.md", "<<<<<<< local (skiman)\nmine\n");
    up.commit("v2");
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(1)
        .stdout_eq(str![[r#"
skill pdf from https://github.com/acme/pdf.git at [..] (default branch main):
...
"#]])
        .stderr_eq(str![[r#"
error: refusing to write content that breaks the rules:
  skills/pdf/notes.md:1: unresolved-conflict: conflict marker from an update; resolve the conflict and remove the markers

"#]]);

    assert_eq!(world(&sandbox), before);
}

#[test]
fn a_violation_elsewhere_in_the_skill_repo_is_only_a_warning() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let broken = sandbox.skill_repo().join("skills/notes/SKILL.md");
    std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
    std::fs::write(&broken, "no frontmatter\n").unwrap();
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0)
        .stderr_eq(str![[r#"
warning: skills/notes/SKILL.md:1: frontmatter: the frontmatter must start on line 1 with `---`

"#]]);

    assert!(sandbox.skill_repo().join("skills/pdf/SKILL.md").is_file());
}

#[test]
fn sources_that_are_not_github_shorthand_are_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let before = world(&sandbox);
    for (source, message) in [
        (
            "-uhelper",
            "error: the source `-uhelper` starts with `-`; sources never do\n",
        ),
        (
            "ext::sh -c touch% /tmp/pwned",
            "error: the source `ext::sh -c touch% /tmp/pwned` uses a git remote helper (`::`), which skiman never runs\n",
        ),
        (
            "file:///tmp/repo",
            "error: unsupported source `file:///tmp/repo`: give a GitHub repository as <owner>/<repo>\n",
        ),
    ] {
        sandbox
            .skiman()
            .args(["add", "--yes", "--", source])
            .assert()
            .code(1)
            .stdout_eq("")
            .stderr_eq(message);
    }
    assert_eq!(world(&sandbox), before);
}

#[test]
fn an_upstream_symlink_is_refused_and_nothing_is_written() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    up.symlink("passwd", "../../../etc/passwd");
    up.commit("v2");
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
error: https://github.com/acme/pdf.git holds a symlink at passwd; symlinks are not allowed in skills

"#]]);

    assert_eq!(world(&sandbox), before);
}

#[test]
fn submodules_are_not_fetched_and_are_listed() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = single_skill(&sandbox, "pdf", PDF_SKILL_MD);
    let head = up.head();
    up.git([
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},vendor/lib"),
    ]);
    up.git(["commit", "--quiet", "-m", "submodule"]);

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skill pdf from https://github.com/acme/pdf.git at [..] (default branch main):
  SKILL.md
  reference.md
  scripts/fill.sh  [script, executable]
  vendor/lib  [submodule, not fetched]
...
"#]]);

    assert!(!sandbox.skill_repo().join("skills/pdf/vendor").exists());
}

#[test]
fn add_ignores_the_git_environment_of_a_calling_git_process() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    single_skill(&sandbox, "pdf", PDF_SKILL_MD);

    sandbox
        .skiman()
        .env("GIT_DIR", sandbox.root().join("nowhere.git"))
        .env("GIT_INDEX_FILE", sandbox.root().join("nowhere.index"))
        .env("GIT_WORK_TREE", sandbox.root())
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);

    assert!(sandbox.skill_repo().join("upstream/pdf/SKILL.md").is_file());
}
