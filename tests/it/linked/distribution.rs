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
linked [..]/skill-repo/linked/company
linked [..]/skill-repo/linked/solo
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

    let linked = sandbox.skill_repo().join("linked");
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
