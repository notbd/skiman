//! Project items that overlap a linked folder (spec "Project items"): the
//! mirror never copies linked content and `project restore` never writes
//! into a linked folder or a target of one of its child links.

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;

use super::{COMPANY, append_manifest, contents};
use crate::support::{Sandbox, Tool, exists, init, read, skill_md, write};

const OLD: &str = "# old\n";
const NEW: &str = "# new\n";

fn work(sandbox: &Sandbox, rel: &str) -> PathBuf {
    sandbox.home().join("work").join(rel)
}

fn mirror(sandbox: &Sandbox, rel: &str) -> PathBuf {
    sandbox.canon().join("projects/work").join(rel)
}

/// The project root `~/work` with `files` (each holding [`OLD`]) and the
/// skill folders `skills`, all adopted into the mirror.
fn adopted(files: &[&str], skills: &[&str]) -> Sandbox {
    let sandbox = Sandbox::new();
    init(&sandbox);
    for rel in files {
        write(work(&sandbox, rel), OLD);
    }
    for rel in skills {
        write(work(&sandbox, rel).join("SKILL.md"), skill_md("solo"));
    }
    sandbox
        .skiman()
        .args(["project", "add"])
        .arg(work(&sandbox, ""))
        .assert()
        .code(0);
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    sandbox
}

fn run(sandbox: &Sandbox, args: &[&str], paths: &[PathBuf]) -> (i32, String) {
    let output = sandbox.skiman().args(args).args(paths).output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// Items inside the registered folder, inside a child link's target (a
/// folder, a dangling target, the missing end of a chain of two links), and
/// a project skill that is itself a registered folder: none is mirrored
/// or restored, while the root's other items are.
#[test]
fn items_that_overlap_a_linked_folder_are_neither_mirrored_nor_restored() {
    let files = [
        "AGENTS.md",
        "company/AGENTS.md",
        "ext/AGENTS.md",
        "gone/AGENTS.md",
        "final/AGENTS.md",
    ];
    let sandbox = adopted(&files, &["proj/.claude/skills/solo"]);
    let company = work(&sandbox, "company");
    write(company.join("alpha/SKILL.md"), skill_md("alpha"));
    write(work(&sandbox, "ext/SKILL.md"), skill_md("ext"));
    symlink(work(&sandbox, "ext"), company.join("ext")).unwrap();
    symlink(work(&sandbox, "gone"), company.join("gone")).unwrap();
    symlink(work(&sandbox, "hop"), company.join("chain")).unwrap();
    symlink(work(&sandbox, "final"), work(&sandbox, "hop")).unwrap();
    append_manifest(
        &sandbox,
        "\n[linked.company]\npath = \"~/work/company\"\n\n[linked.solo]\npath = \"~/work/proj/.claude/skills/solo\"\n",
    );
    std::fs::remove_dir_all(work(&sandbox, "gone")).unwrap();
    std::fs::remove_dir_all(work(&sandbox, "final")).unwrap();
    for rel in ["AGENTS.md", "company/AGENTS.md", "ext/AGENTS.md"] {
        std::fs::write(work(&sandbox, rel), NEW).unwrap();
    }
    std::fs::write(work(&sandbox, "proj/.claude/skills/solo/notes.md"), NEW).unwrap();
    let (ext, proj) = (work(&sandbox, "ext"), work(&sandbox, "proj"));
    let watched = [company.as_path(), ext.as_path(), proj.as_path()];
    let before = contents(&watched);

    let (code, stderr) = run(&sandbox, &["sync"], &[]);

    assert_eq!(code, 0, "{stderr}");
    assert_eq!(read(mirror(&sandbox, "AGENTS.md")), NEW);
    for rel in ["company/AGENTS.md", "ext/AGENTS.md"] {
        assert_eq!(read(mirror(&sandbox, rel)), OLD, "{rel}");
        let line = format!(
            "{} is not synced: it overlaps linked folder `company`",
            work(&sandbox, rel).display()
        );
        assert!(stderr.contains(&line), "{line}\n{stderr}");
    }
    assert!(!exists(mirror(
        &sandbox,
        "proj/.claude/skills/solo/notes.md"
    )));
    let synced = read(sandbox.state_home().join("skiman/projects-synced.toml"));
    assert!(synced.contains("company/AGENTS.md"), "{synced}");
    for tool in Tool::ALL {
        assert!(sandbox.tool_dir(tool).join("alpha/SKILL.md").is_file());
        assert!(sandbox.tool_dir(tool).join("solo/SKILL.md").is_file());
    }

    let exact: Vec<PathBuf> = ["gone/AGENTS.md", "final/AGENTS.md", "company/AGENTS.md"]
        .iter()
        .map(|rel| work(&sandbox, rel))
        .collect();
    let (code, stderr) = run(&sandbox, &["project", "restore", "--yes"], &exact);
    assert_eq!(code, 1, "{stderr}");
    for path in &exact {
        assert!(
            stderr.contains(&format!(
                "skip {}: it overlaps linked folder `company`",
                path.display()
            )),
            "{stderr}"
        );
    }
    assert!(!exists(work(&sandbox, "gone")) && !exists(work(&sandbox, "final")));
    for rel in ["gone/AGENTS.md", "final/AGENTS.md"] {
        assert_eq!(read(mirror(&sandbox, rel)), OLD, "{rel}");
    }
    let (code, stderr) = run(
        &sandbox,
        &["project", "adopt", "--yes"],
        &[work(&sandbox, "")],
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(read(mirror(&sandbox, "company/AGENTS.md")), OLD);
    assert_eq!(contents(&watched), before);
}

/// A table that is not well formed is skipped, but its path still keeps
/// project items inside it from being mirrored or restored.
#[test]
fn items_inside_a_malformed_registration_are_neither_mirrored_nor_restored() {
    let sandbox = adopted(&["AGENTS.md", "company/AGENTS.md"], &[]);
    let company = work(&sandbox, "company");
    write(company.join("alpha/SKILL.md"), skill_md("alpha"));
    append_manifest(
        &sandbox,
        "\n[linked.company]\npath = \"~/work/company\"\nnote = \"x\"\n",
    );
    for rel in ["AGENTS.md", "company/AGENTS.md"] {
        std::fs::write(work(&sandbox, rel), NEW).unwrap();
    }

    let (code, stderr) = run(&sandbox, &["sync"], &[]);

    assert_eq!(code, 3, "{stderr}");
    assert_eq!(read(mirror(&sandbox, "AGENTS.md")), NEW);
    assert_eq!(read(mirror(&sandbox, "company/AGENTS.md")), OLD);
    let item = work(&sandbox, "company/AGENTS.md");
    let line = format!(
        "{} is not synced: it overlaps linked folder `company`",
        item.display()
    );
    assert!(stderr.contains(&line), "{line}\n{stderr}");
    let before = contents(&[&company]);
    let (code, stderr) = run(
        &sandbox,
        &["project", "restore", "--yes"],
        std::slice::from_ref(&item),
    );
    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.contains(&format!(
            "skip {}: it overlaps linked folder `company`",
            item.display()
        )),
        "{stderr}"
    );
    assert_eq!(contents(&[&company]), before);
}

/// While a registered folder cannot be listed its child links are unknown:
/// project items are not mirrored, adopted or restored, and distribution
/// goes on.
#[test]
fn an_unlistable_folder_pauses_project_items_but_not_distribution() {
    let sandbox = adopted(&["AGENTS.md"], &[]);
    let company = work(&sandbox, "company");
    write(company.join("alpha/SKILL.md"), skill_md("alpha"));
    append_manifest(&sandbox, COMPANY);
    write(
        sandbox.canon().join("skills/notes/SKILL.md"),
        skill_md("notes"),
    );
    std::fs::write(work(&sandbox, "AGENTS.md"), NEW).unwrap();
    std::fs::set_permissions(&company, std::fs::Permissions::from_mode(0o000)).unwrap();

    let synced = run(&sandbox, &["sync"], &[]);
    let adopt = run(&sandbox, &["project", "adopt", "--yes"], &[]);
    let restore = run(
        &sandbox,
        &["project", "restore", "--yes"],
        &[work(&sandbox, "AGENTS.md")],
    );
    std::fs::set_permissions(&company, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(synced.0, 3, "{}", synced.1);
    assert!(
        synced
            .1
            .contains("project items are not synced while linked folder `company` cannot be read"),
        "{}",
        synced.1
    );
    assert!(
        sandbox
            .tool_dir(Tool::ClaudeCode)
            .join("notes/SKILL.md")
            .is_file()
    );
    for (code, stderr) in [adopt, restore] {
        assert_eq!(code, 3, "{stderr}");
        assert!(
            stderr.contains("linked folder `company` cannot be read"),
            "{stderr}"
        );
    }
    assert_eq!(read(mirror(&sandbox, "AGENTS.md")), OLD);
    assert_eq!(read(work(&sandbox, "AGENTS.md")), NEW);
    assert_eq!(run(&sandbox, &["sync"], &[]).0, 0);
    assert_eq!(read(mirror(&sandbox, "AGENTS.md")), NEW);
}

/// A child link `home -> ~` breaks the path rules, so it protects nothing:
/// project items under `~` mirror and restore as usual.
#[test]
fn a_child_link_to_home_protects_nothing() {
    let sandbox = adopted(&["AGENTS.md"], &[]);
    let company = sandbox.home().join("company");
    write(company.join("alpha/SKILL.md"), skill_md("alpha"));
    symlink(sandbox.home(), company.join("home")).unwrap();
    append_manifest(&sandbox, "\n[linked.company]\npath = \"~/company\"\n");
    std::fs::write(work(&sandbox, "AGENTS.md"), NEW).unwrap();

    assert_eq!(run(&sandbox, &["sync"], &[]).0, 0);
    assert_eq!(read(mirror(&sandbox, "AGENTS.md")), NEW);
    std::fs::remove_file(work(&sandbox, "AGENTS.md")).unwrap();
    let (code, stderr) = run(
        &sandbox,
        &["project", "restore", "--yes"],
        &[work(&sandbox, "AGENTS.md")],
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(read(work(&sandbox, "AGENTS.md")), NEW);
}

/// A registered path that is not on this machine, its parent missing too,
/// overlaps no other project item.
#[test]
fn a_missing_registered_path_keeps_every_other_item_working() {
    let sandbox = adopted(&["AGENTS.md", "sub/AGENTS.md"], &[]);
    append_manifest(
        &sandbox,
        "\n[linked.company]\npath = \"~/work/nowhere/company\"\n",
    );
    std::fs::write(work(&sandbox, "sub/AGENTS.md"), NEW).unwrap();

    assert_eq!(run(&sandbox, &["sync"], &[]).0, 0);
    assert_eq!(read(mirror(&sandbox, "sub/AGENTS.md")), NEW);
    std::fs::remove_file(work(&sandbox, "AGENTS.md")).unwrap();
    let (code, stderr) = run(
        &sandbox,
        &["project", "restore", "--yes"],
        &[work(&sandbox, "AGENTS.md")],
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(read(work(&sandbox, "AGENTS.md")), OLD);
}

/// A child link to a regular file is not a skill and protects nothing: the
/// project item it points at is adopted as usual.
#[test]
fn a_child_link_to_a_file_does_not_block_its_project_item() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let agents = work(&sandbox, "client/AGENTS.md");
    write(&agents, OLD);
    let company = work(&sandbox, "company");
    write(company.join("alpha/SKILL.md"), skill_md("alpha"));
    symlink("../client/AGENTS.md", company.join("README.md")).unwrap();
    append_manifest(&sandbox, COMPANY);
    let root = work(&sandbox, "");
    assert_eq!(run(&sandbox, &["project", "add"], &[root]).0, 0);

    let (code, stderr) = run(&sandbox, &["project", "adopt", "--yes"], &[agents]);

    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains("overlaps linked folder"), "{stderr}");
    assert_eq!(read(mirror(&sandbox, "client/AGENTS.md")), OLD);
}
