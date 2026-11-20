//! Reading a linked folder and distributing its skills (spec "Reading a
//! linked folder", "Sync"): `<canon>/linked/<id>` points at the registered
//! path, and every tool directory links each linked skill through it.

use snapbox::str;

use super::{append_manifest, contents, folder};
use crate::support::{Sandbox, Tool, assert_symlink, exists, init, write};

/// A canon with the folder `~/work/company` (skills `alpha` and `beta`)
/// registered as `company`, and the single skill folder `~/work/solo`
/// registered as `solo`.
fn two_folders() -> Sandbox {
    let sandbox = Sandbox::new();
    init(&sandbox);
    folder(&sandbox, "work/company", &["alpha", "beta"]);
    folder(&sandbox, "work", &["solo"]);
    append_manifest(
        &sandbox,
        "\n[linked.company]\npath = \"~/work/company\"\n\n[linked.solo]\npath = \"~/work/solo\"\n",
    );
    sandbox
}

#[test]
fn a_folder_of_skills_and_a_single_skill_folder_reach_every_tool_directory() {
    let sandbox = two_folders();
    let home = sandbox.home();
    let before = contents(&[&home.join("work")]);

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
linked [..]/canon/linked/company
linked [..]/canon/linked/solo
linked [..]/home/.claude/skills/alpha
linked [..]/home/.claude/skills/beta
linked [..]/home/.claude/skills/solo
linked [..]/home/.agents/skills/alpha
linked [..]/home/.agents/skills/beta
linked [..]/home/.agents/skills/solo
linked [..]/home/.gemini/config/skills/alpha
linked [..]/home/.gemini/config/skills/beta
linked [..]/home/.gemini/config/skills/solo

"#]])
        .stderr_eq("");

    let linked = sandbox.canon().join("linked");
    assert_symlink(
        linked.join("company"),
        home.join("work/company"),
        home.join("work/company"),
    );
    assert_symlink(
        linked.join("solo"),
        home.join("work/solo"),
        home.join("work/solo"),
    );
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        for name in ["alpha", "beta"] {
            assert_symlink(
                dir.join(name),
                linked.join("company").join(name),
                home.join("work/company").join(name),
            );
        }
        assert_symlink(
            dir.join("solo"),
            linked.join("solo"),
            home.join("work/solo"),
        );
    }
    assert_eq!(contents(&[&home.join("work")]), before);
    sandbox.skiman().arg("sync").assert().code(0).stdout_eq("");
}

#[test]
fn a_skill_added_to_or_deleted_from_the_folder_is_followed_by_the_next_sync() {
    let sandbox = two_folders();
    sandbox.skiman().arg("sync").assert().code(0);
    let company = sandbox.home().join("work/company");
    write(
        company.join("gamma/SKILL.md"),
        crate::support::skill_md("gamma"),
    );
    std::fs::remove_dir_all(company.join("alpha")).unwrap();

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("");

    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert!(!exists(dir.join("alpha")), "{}", tool.tool_dir());
        assert!(dir.join("gamma/SKILL.md").is_file());
    }
}

/// Problems among a folder's entries are its own content's: each is
/// reported with exit 0 and the rest of the folder, the canon's skills and
/// the other linked folders sync as usual.
#[test]
fn reserved_invalid_and_dot_entries_are_not_distributed() {
    let sandbox = two_folders();
    let company = sandbox.home().join("work/company");
    for name in ["skiman-manual", "Bad_Name", ".hidden"] {
        write(
            company.join(name).join("SKILL.md"),
            crate::support::skill_md(name),
        );
    }
    write(
        sandbox.canon().join("skills/notes/SKILL.md"),
        crate::support::skill_md("notes"),
    );

    let output = sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    for name in ["skiman-manual", "Bad_Name"] {
        let line = format!(
            "warning: {} is a linked skill folder that is not distributed: ",
            company.join(name).display()
        );
        assert!(stderr.contains(&line), "{stderr}");
    }
    assert!(!stderr.contains(".hidden"), "{stderr}");
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        for name in ["alpha", "beta", "solo", "notes"] {
            assert!(dir.join(name).join("SKILL.md").is_file(), "{name}");
        }
        assert!(!exists(dir.join("Bad_Name")) && !exists(dir.join(".hidden")));
        let manual = std::fs::read_link(dir.join("skiman-manual")).unwrap();
        assert!(manual.ends_with(".skiman/skiman-manual"), "{manual:?}");
    }
}

/// A child link that loops and one whose target is gone are no skills: each
/// is reported (exit 0), and the dangling one's old link goes.
#[test]
fn a_looping_or_dangling_child_link_is_reported_and_not_distributed() {
    let sandbox = two_folders();
    let elsewhere = sandbox.root().join("elsewhere");
    write(
        elsewhere.join("gone/SKILL.md"),
        crate::support::skill_md("gone"),
    );
    let company = sandbox.home().join("work/company");
    std::os::unix::fs::symlink(elsewhere.join("gone"), company.join("gone")).unwrap();
    std::os::unix::fs::symlink("loop", company.join("loop")).unwrap();
    sandbox.skiman().arg("sync").assert().code(0);
    assert!(
        sandbox
            .tool_dir(Tool::Codex)
            .join("gone/SKILL.md")
            .is_file()
    );
    std::fs::remove_dir_all(&elsewhere).unwrap();

    let output = sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .get_output()
        .clone();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(&format!(
            "{} is a link that loops",
            company.join("loop").display()
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "{} is a dangling link to",
            company.join("gone").display()
        )),
        "{stderr}"
    );
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert!(!exists(dir.join("gone")) && !exists(dir.join("loop")));
        assert!(dir.join("alpha/SKILL.md").is_file());
    }
}

/// A sub-folder whose `SKILL.md` cannot be checked is never taken for
/// absent: it keeps its link (exit 3), and the rest of the folder syncs.
#[test]
fn an_unreadable_sub_folder_keeps_its_link_and_needs_attention() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = two_folders();
    sandbox.skiman().arg("sync").assert().code(0);
    let company = sandbox.home().join("work/company");
    write(
        company.join("gamma/SKILL.md"),
        crate::support::skill_md("gamma"),
    );
    let alpha = company.join("alpha");
    std::fs::set_permissions(&alpha, std::fs::Permissions::from_mode(0o000)).unwrap();

    let output = sandbox.skiman().arg("sync").output().unwrap();
    std::fs::set_permissions(&alpha, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("linked skill alpha of `company` is not checked"),
        "{stderr}"
    );
    let linked = sandbox.canon().join("linked/company");
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert_eq!(
            std::fs::read_link(dir.join("alpha")).unwrap(),
            linked.join("alpha")
        );
        assert!(dir.join("gamma/SKILL.md").is_file());
        assert!(dir.join("solo/SKILL.md").is_file());
    }
}

/// A top-level `SKILL.md` that cannot be probed leaves the folder's shape
/// unknown, so the whole id is in a read error (exit 3), whether it was a
/// folder of skills or one skill: its links stay exactly as they are, and
/// a rename or a new skill inside it is not followed until it can be read.
#[test]
fn a_top_level_skill_md_that_cannot_be_probed_leaves_the_whole_id_unread() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = two_folders();
    sandbox.skiman().arg("sync").assert().code(0);
    let work = sandbox.home().join("work");
    let locked = work.join("locked");
    write(locked.join("SKILL.md"), crate::support::skill_md("locked"));
    for id in ["company", "solo"] {
        let top = work.join(id).join("SKILL.md");
        if top.exists() {
            std::fs::remove_file(&top).unwrap();
        }
        std::os::unix::fs::symlink(locked.join("SKILL.md"), top).unwrap();
    }
    std::fs::rename(work.join("company/alpha"), work.join("company/gamma")).unwrap();
    write(
        work.join("solo/extra/SKILL.md"),
        crate::support::skill_md("extra"),
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let output = sandbox.skiman().arg("sync").output().unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    for id in ["company", "solo"] {
        let line = format!(
            "linked folder `{id}` is not read (cannot read {}: ",
            work.join(id).join("SKILL.md").display()
        );
        assert!(stderr.contains(&line), "{line}\n{stderr}");
    }
    assert!(!stderr.contains("is not checked"), "{stderr}");
    let linked = sandbox.canon().join("linked");
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert_eq!(
            std::fs::read_link(dir.join("alpha")).unwrap(),
            linked.join("company/alpha")
        );
        assert!(dir.join("beta/SKILL.md").is_file());
        for name in ["gamma", "extra"] {
            assert!(!exists(dir.join(name)), "{name}: {}", dir.display());
        }
        assert_eq!(
            std::fs::read_link(dir.join("solo")).unwrap(),
            linked.join("solo")
        );
    }
}

/// After `linked remove`, no link of the folder is left in the canon or in
/// any tool directory.
#[test]
fn linked_remove_leaves_no_link_behind() {
    let sandbox = two_folders();
    sandbox.skiman().arg("sync").assert().code(0);
    let before = contents(&[&sandbox.home().join("work")]);

    sandbox
        .skiman()
        .args(["linked", "remove", "company"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
unregistered linked folder company: ~/work/company
removed [..]/canon/linked/company
removed [..]/home/.claude/skills/alpha
removed [..]/home/.claude/skills/beta
removed [..]/home/.agents/skills/alpha
removed [..]/home/.agents/skills/beta
removed [..]/home/.gemini/config/skills/alpha
removed [..]/home/.gemini/config/skills/beta

"#]]);

    assert!(!exists(sandbox.canon().join("linked/company")));
    assert!(!crate::support::read(sandbox.canon().join("skiman.toml")).contains("company"));
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert!(!exists(dir.join("alpha")) && !exists(dir.join("beta")));
        assert!(dir.join("solo/SKILL.md").is_file());
    }
    assert_eq!(contents(&[&sandbox.home().join("work")]), before);
}

/// A folder that moved needs only its `path` line: one link in the canon
/// is retargeted and no tool-directory link changes, even for a single
/// skill folder whose name changed with the move.
#[test]
fn a_moved_folder_needs_only_its_path_line() {
    let sandbox = two_folders();
    sandbox.skiman().arg("sync").assert().code(0);
    let tool_links = || {
        Tool::ALL
            .iter()
            .map(|tool| crate::support::tree(sandbox.tool_dir(*tool)))
            .collect::<Vec<_>>()
    };
    let links = tool_links();
    let home = sandbox.home();
    std::fs::create_dir(home.join("moved")).unwrap();
    std::fs::rename(home.join("work/company"), home.join("moved/company")).unwrap();
    std::fs::rename(home.join("work/solo"), home.join("moved/solo-renamed")).unwrap();
    let manifest = sandbox.canon().join("skiman.toml");
    let text = crate::support::read(&manifest)
        .replace("~/work/company", "~/moved/company")
        .replace("~/work/solo", "~/moved/solo-renamed");
    std::fs::write(&manifest, text).unwrap();

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
retargeted [..]/canon/linked/company
retargeted [..]/canon/linked/solo

"#]])
        .stderr_eq("");

    assert_eq!(tool_links(), links);
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert!(dir.join("alpha/SKILL.md").is_file() && dir.join("solo/SKILL.md").is_file());
    }
}
