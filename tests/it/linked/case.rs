//! Paths that differ from the disk only in letter case (spec "Path
//! rules"): on a volume that ignores case (macOS APFS by default) such a
//! path names the same folder, so every rule holds for it as for the path
//! spelled as on disk. On a case-sensitive volume the two spellings are
//! two folders and these tests have nothing to show, so they skip.

use std::os::unix::fs::symlink;
use std::path::Path;

use super::{append_manifest, contents, folder, sync};
use crate::support::{Sandbox, Tool, init, read, write};

/// Whether the volume under `root` ignores letter case.
fn case_insensitive(root: &Path) -> bool {
    let probe = root.join("CASE-PROBE");
    std::fs::create_dir(&probe).unwrap();
    let yes = root.join("case-probe").exists();
    std::fs::remove_dir(&probe).unwrap();
    yes
}

/// A sandbox with a canon, or `None` (and a note) on a case-sensitive volume.
#[allow(clippy::print_stdout)]
fn sandbox() -> Option<Sandbox> {
    let sandbox = Sandbox::new();
    if !case_insensitive(sandbox.root()) {
        println!("skipped: the sandbox's volume is case-sensitive");
        return None;
    }
    init(&sandbox);
    Some(sandbox)
}

fn run(sandbox: &Sandbox, args: &[&str]) -> (i32, String) {
    let output = sandbox.skiman().args(args).output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// A project item adopted as `~/work/company/AGENTS.md` inside a folder
/// registered as `~/work/COMPANY`: restore leaves it alone.
#[test]
fn restore_writes_nothing_into_a_folder_registered_in_another_case() {
    let Some(sandbox) = sandbox() else { return };
    let work = sandbox.home().join("work");
    let item = work.join("company/AGENTS.md");
    write(&item, "old content\n");
    sandbox
        .skiman()
        .args(["project", "add"])
        .arg(&work)
        .assert()
        .code(0);
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .arg(&item)
        .assert()
        .code(0);
    append_manifest(&sandbox, "\n[linked.company]\npath = \"~/work/COMPANY\"\n");
    write(
        sandbox.canon().join("projects/work/company/AGENTS.md"),
        "changed mirror\n",
    );

    let output = sandbox
        .skiman()
        .args(["project", "restore", "--yes"])
        .arg(&item)
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("overlaps linked folder `company`"),
        "{stderr}"
    );
    assert_eq!(read(&item), "old content\n");
}

/// A tool directory that is a link into a folder registered in another
/// case: sync writes nothing through it (exit 3).
#[test]
fn sync_writes_nothing_through_a_tool_directory_into_a_folder_registered_in_another_case() {
    let Some(sandbox) = sandbox() else { return };
    write(
        sandbox.canon().join("skills/notes/SKILL.md"),
        crate::support::skill_md("notes"),
    );
    let other = folder(&sandbox, "work/other", &["alpha"]);
    append_manifest(&sandbox, "\n[linked.other]\npath = \"~/work/Other\"\n");
    let codex = sandbox.tool_dir(Tool::Codex);
    std::fs::remove_dir_all(&codex).unwrap();
    std::fs::create_dir(other.join("agents-skills")).unwrap();
    symlink(other.join("agents-skills"), &codex).unwrap();
    let before = contents(&[&other]);

    let (code, stderr) = sync(&sandbox);

    assert_eq!(code, 3, "{stderr}");
    assert!(stderr.contains("inside linked folder `other`"), "{stderr}");
    assert_eq!(contents(&[&other]), before);
}

/// `~/.CLAUDE` is Claude Code's presence directory: `linked add` refuses it.
#[test]
fn linked_add_refuses_a_place_skiman_writes_spelled_in_another_case() {
    let Some(sandbox) = sandbox() else { return };
    let manifest = sandbox.canon().join("skiman.toml");
    let before = read(&manifest);

    let output = sandbox
        .skiman()
        .args(["linked", "add", "--id", "claude-cap"])
        .arg(sandbox.home().join(".CLAUDE"))
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("presence directory"), "{stderr}");
    assert_eq!(read(&manifest), before);
    assert!(!sandbox.canon().join("linked/claude-cap").exists());
}

/// An older script's link into a folder registered in another case gets no
/// adopt hint, and adopt refuses to copy it into the canon.
#[test]
fn adopt_refuses_a_link_into_a_folder_registered_in_another_case() {
    let Some(sandbox) = sandbox() else { return };
    let company = folder(&sandbox, "Work/Company", &["pdf"]);
    append_manifest(&sandbox, "\n[linked.company]\npath = \"~/work/company\"\n");
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    symlink(company.join("pdf"), claude.join("old")).unwrap();

    let (code, stderr) = sync(&sandbox);
    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains("skiman adopt old"), "{stderr}");
    assert!(!stderr.contains("skiman linked add"), "{stderr}");

    let canon = crate::support::tree(sandbox.canon());
    let (code, stderr) = run(
        &sandbox,
        &["adopt", "old", "--from", "claude-code", "--yes"],
    );
    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.contains("resolves into linked folder `company`"),
        "{stderr}"
    );
    assert_eq!(crate::support::tree(sandbox.canon()), canon);
}

/// A link met on the way, typed in another case, is compared as spelled
/// on disk: `~/.AGENTS/extra` goes through the link `~/.agents`, which
/// holds Codex's tool directory, like `~/.agents/extra` does.
#[test]
fn a_link_on_the_way_typed_in_another_case_breaks_the_rules_as_on_disk() {
    let Some(sandbox) = sandbox() else { return };
    let agents = sandbox.home().join(".agents");
    let real = folder(&sandbox, "agents-real/extra", &["alpha"])
        .parent()
        .unwrap()
        .to_owned();
    std::fs::rename(&agents, real.join("old")).unwrap();
    std::fs::rename(real.join("old/skills"), real.join("skills")).unwrap();
    symlink(&real, &agents).unwrap();
    let manifest = sandbox.canon().join("skiman.toml");
    let before = read(&manifest);

    for typed in [".agents/extra", ".AGENTS/extra"] {
        let output = sandbox
            .skiman()
            .args(["linked", "add", "--id", "extra"])
            .arg(sandbox.home().join(typed))
            .output()
            .unwrap();

        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(1), "{typed}: {stderr}");
        assert!(
            stderr.contains("goes through the link"),
            "{typed}: {stderr}"
        );
        assert_eq!(read(&manifest), before, "{typed}");
    }
}

/// A top-level `skill.md` is the folder's `SKILL.md` on such a volume: when
/// it cannot be probed it is reported once, as the single-skill shape, and
/// never also as an entry of the folder.
#[test]
fn a_top_level_skill_md_in_another_case_is_not_an_entry() {
    use std::os::unix::fs::PermissionsExt;
    let Some(sandbox) = sandbox() else {
        return;
    };
    let company = folder(&sandbox, "work/company", &["alpha"]);
    let locked = sandbox.home().join("work/locked");
    write(locked.join("SKILL.md"), "x");
    symlink(locked.join("SKILL.md"), company.join("skill.md")).unwrap();
    append_manifest(&sandbox, "\n[linked.company]\npath = \"~/work/company\"\n");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let (code, stderr) = sync(&sandbox);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(code, 3, "{stderr}");
    assert!(
        stderr.contains("linked skill company of `company` is not checked"),
        "{stderr}"
    );
    assert_eq!(stderr.matches("is not checked").count(), 1, "{stderr}");
    assert!(
        sandbox
            .tool_dir(Tool::ClaudeCode)
            .join("alpha/SKILL.md")
            .is_file()
    );
}
