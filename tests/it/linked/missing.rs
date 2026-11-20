//! Missing and unreadable linked folders (spec "Sync"): a folder that is
//! not on this machine gives its names up until it is back; one that
//! cannot be read keeps everything as it is, so a passing failure never
//! withdraws a skill or changes a collision winner.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::{COMPANY, append_manifest, folder};
use crate::support::{Sandbox, Tool, exists, init, read, skill_md, write};

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn sync(sandbox: &Sandbox, args: &[&str]) -> (i32, String, String) {
    let output = sandbox.skiman().args(args).output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

fn set_path(sandbox: &Sandbox, from: &str, to: &str) {
    let manifest = sandbox.canon().join("skiman.toml");
    std::fs::write(&manifest, read(&manifest).replace(from, to)).unwrap();
}

/// A canon with `~/work/company` (skills `alpha` and `beta`) registered and synced.
fn company() -> Sandbox {
    let sandbox = Sandbox::new();
    init(&sandbox);
    folder(&sandbox, "work/company", &["alpha", "beta"]);
    append_manifest(&sandbox, COMPANY);
    assert_eq!(sync(&sandbox, &["sync"]).0, 0);
    sandbox
}

#[test]
fn a_missing_folder_is_reported_once_its_links_go_and_come_back_with_it() {
    let sandbox = company();
    let home = sandbox.home();
    std::fs::rename(home.join("work/company"), home.join("away")).unwrap();

    let (code, stdout, _) = sync(&sandbox, &["sync", "--hook"]);
    assert_eq!(code, 0);
    let missing = |path: &str| {
        format!(
            "skiman: linked folder `company` ({}) is not on this machine, so its skills are not distributed\n",
            home.join(path).display()
        )
    };
    assert_eq!(stdout, missing("work/company"));
    assert!(!exists(sandbox.canon().join("linked/company")));
    for tool in Tool::ALL {
        assert!(!exists(sandbox.tool_dir(tool).join("alpha")));
    }
    assert_eq!(sync(&sandbox, &["sync", "--hook"]).1, "");

    set_path(&sandbox, "~/work/company", "~/elsewhere/company");
    assert_eq!(
        sync(&sandbox, &["sync", "--hook"]).1,
        missing("elsewhere/company")
    );
    assert_eq!(sync(&sandbox, &["sync"]).0, 0);

    set_path(&sandbox, "~/elsewhere/company", "~/work/company");
    std::fs::rename(home.join("away"), home.join("work/company")).unwrap();
    assert_eq!(
        sync(&sandbox, &["sync", "--hook"]),
        (0, String::new(), String::new())
    );
    for tool in Tool::ALL {
        assert!(sandbox.tool_dir(tool).join("alpha/SKILL.md").is_file());
    }
}

/// While the folder cannot be read (not listed, or listed but not
/// entered), its links and names stay, even against a folder whose id
/// sorts first; everything else syncs, and disabling a tool still removes
/// the links there. The next good sync converges.
#[test]
fn an_unreadable_folder_keeps_its_links_and_names_until_it_can_be_read() {
    // Not listed or entered; listed, not entered; entered, not listed.
    for mode in [0o000, 0o444, 0o111] {
        let sandbox = company();
        let company = sandbox.home().join("work/company");
        let linked = sandbox.canon().join("linked");
        folder(&sandbox, "work/acme", &["alpha"]);
        append_manifest(&sandbox, "\n[linked.acme]\npath = \"~/work/acme\"\n");
        write(
            sandbox.canon().join("skills/notes/SKILL.md"),
            skill_md("notes"),
        );
        set_path(
            &sandbox,
            "enabled = [\"claude-code\", \"codex\", \"antigravity\"]",
            "enabled = [\"claude-code\", \"codex\"]",
        );
        chmod(&company, mode);

        let (code, _, stderr) = sync(&sandbox, &["sync"]);
        chmod(&company, 0o755);

        assert_eq!(code, 3, "{mode:o}: {stderr}");
        assert!(
            stderr.contains("linked folder `company` is not read ("),
            "{stderr}"
        );
        assert!(stderr.contains("linked skill alpha in "), "{stderr}");
        if mode == 0o000 {
            // A folder that cannot be listed is named itself, not its SKILL.md.
            assert!(stderr.contains("/company: "), "{stderr}");
        }
        assert_eq!(std::fs::read_link(linked.join("company")).unwrap(), company);
        for tool in [Tool::ClaudeCode, Tool::Codex] {
            let dir = sandbox.tool_dir(tool);
            for name in ["alpha", "beta"] {
                assert_eq!(
                    std::fs::read_link(dir.join(name)).unwrap(),
                    linked.join("company").join(name),
                    "{mode:o}"
                );
            }
            assert!(dir.join("notes/SKILL.md").is_file());
        }
        let antigravity = sandbox.tool_dir(Tool::Antigravity);
        assert!(!exists(antigravity.join("alpha")) && !exists(antigravity.join("beta")));

        let (code, _, stderr) = sync(&sandbox, &["sync"]);
        assert_eq!(code, 3, "{stderr}");
        let dir = sandbox.tool_dir(Tool::ClaudeCode);
        assert_eq!(
            std::fs::read_link(dir.join("alpha")).unwrap(),
            linked.join("acme/alpha")
        );
        assert_eq!(
            std::fs::read_link(dir.join("beta")).unwrap(),
            linked.join("company/beta")
        );
    }
}

/// A canon skill that appears while the folder whose kept links hold its
/// name cannot be read gets that name in no tool directory, not even one
/// where the kept link is missing, and wins it over no other linked skill;
/// skiman's link to it that is already there stays. Sync, `check` and
/// `doctor` say why. Once the folder reads again, the canon skill wins the
/// name as usual.
#[test]
fn an_unreadable_folder_keeps_its_names_from_a_canon_skill_in_every_tool_directory() {
    let sandbox = company();
    let company = sandbox.home().join("work/company");
    let linked = sandbox.canon().join("linked/company");
    let acme = folder(&sandbox, "work/acme", &["alpha"]);
    append_manifest(&sandbox, "\n[linked.acme]\npath = \"~/work/acme\"\n");
    for name in ["alpha", "notes"] {
        write(
            sandbox.canon().join("skills").join(name).join("SKILL.md"),
            skill_md(name),
        );
    }
    let canon_alpha = sandbox.canon().join("skills/alpha");
    let (claude, antigravity) = (
        sandbox.tool_dir(Tool::ClaudeCode),
        sandbox.tool_dir(Tool::Antigravity),
    );
    std::fs::remove_file(claude.join("alpha")).unwrap();
    std::fs::remove_file(antigravity.join("alpha")).unwrap();
    std::os::unix::fs::symlink(&canon_alpha, antigravity.join("alpha")).unwrap();
    chmod(&company, 0o000);

    let (code, _, stderr) = sync(&sandbox, &["sync"]);
    let vacant = exists(claude.join("alpha"));
    let said = ["check", "doctor"].map(|command| {
        let output = sandbox.skiman().arg(command).output().unwrap();
        String::from_utf8(output.stdout).unwrap() + &String::from_utf8(output.stderr).unwrap()
    });
    chmod(&company, 0o755);

    assert_eq!(code, 3, "{stderr}");
    let kept = "linked folder `company`, which cannot be read now, keeps the name";
    let withheld = format!("skill folder skills/alpha is not distributed: {kept}");
    let lost = format!(
        "linked skill alpha in {} is not distributed: {kept}",
        acme.join("alpha").display()
    );
    for out in [&stderr, &said[0], &said[1]] {
        assert!(out.contains(&withheld), "{withheld}\n{out}");
        assert!(out.contains(&lost), "{lost}\n{out}");
        assert!(!out.contains("skills/alpha uses the name"), "{out}");
    }
    assert!(!vacant, "{stderr}");
    assert_eq!(
        std::fs::read_link(sandbox.tool_dir(Tool::Codex).join("alpha")).unwrap(),
        linked.join("alpha")
    );
    assert_eq!(
        std::fs::read_link(antigravity.join("alpha")).unwrap(),
        canon_alpha
    );
    for tool in Tool::ALL {
        assert!(sandbox.tool_dir(tool).join("notes/SKILL.md").is_file());
    }

    let (code, _, stderr) = sync(&sandbox, &["sync"]);
    assert_eq!(code, 3, "{stderr}");
    assert!(
        stderr.contains("is not distributed: the skill folder skills/alpha uses the name"),
        "{stderr}"
    );
    for tool in Tool::ALL {
        assert_eq!(
            std::fs::read_link(sandbox.tool_dir(tool).join("alpha")).unwrap(),
            canon_alpha
        );
    }
}

/// A disabled canon skill under a kept name is not distributed anyway:
/// sync, `check` and `doctor` all leave it out of the withheld report.
#[test]
fn a_disabled_canon_skill_under_a_kept_name_is_not_reported_as_withheld() {
    let sandbox = company();
    let company = sandbox.home().join("work/company");
    write(
        sandbox.canon().join("skills/alpha/SKILL.md"),
        skill_md("alpha"),
    );
    append_manifest(&sandbox, "\n[skills.alpha]\nenabled = false\n");
    chmod(&company, 0o000);

    let said = ["sync", "check", "doctor"].map(|command| {
        let output = sandbox.skiman().arg(command).output().unwrap();
        String::from_utf8(output.stdout).unwrap() + &String::from_utf8(output.stderr).unwrap()
    });
    chmod(&company, 0o755);

    for out in said {
        assert!(!out.contains("skill folder skills/alpha"), "{out}");
    }
}

/// A canon skill folder under a kept name that sync does not distribute
/// anyway (no `SKILL.md`, its name in two group folders, or a symlink) is
/// never a withheld one: sync and `check` report it as invalid, and none
/// of sync, `check` and `doctor` reports it as withheld.
#[test]
fn an_invalid_canon_skill_folder_under_a_kept_name_is_not_reported_as_withheld() {
    let sandbox = company();
    let company = sandbox.home().join("work/company");
    folder(&sandbox, "work/company", &["gamma"]);
    assert_eq!(sync(&sandbox, &["sync"]).0, 0);
    write(sandbox.canon().join("skills/alpha/notes.md"), "notes\n");
    let elsewhere = folder(&sandbox, "elsewhere", &["gamma"]);
    std::os::unix::fs::symlink(
        elsewhere.join("gamma"),
        sandbox.canon().join("skills/gamma"),
    )
    .unwrap();
    for group in ["one", "two"] {
        write(
            sandbox
                .canon()
                .join("skills")
                .join(group)
                .join("beta/SKILL.md"),
            skill_md("beta"),
        );
    }
    chmod(&company, 0o000);

    let said = ["sync", "check", "doctor"].map(|command| {
        let output = sandbox.skiman().arg(command).output().unwrap();
        String::from_utf8(output.stdout).unwrap() + &String::from_utf8(output.stderr).unwrap()
    });
    chmod(&company, 0o755);

    for out in &said {
        assert!(!out.contains("is not distributed: linked folder"), "{out}");
    }
    let canon = sandbox.canon();
    let invalid = |path: &str, why: &str| {
        format!(
            "skill folder {} is not distributed: {why}",
            canon.join(path).display()
        )
    };
    for line in [
        invalid("skills/alpha", "it holds no SKILL.md"),
        invalid(
            "skills/one/beta",
            "its name is in more than one group folder",
        ),
    ] {
        assert!(said[0].contains(&line), "{line}\n{}", said[0]);
    }
    for rule in [
        "skills/alpha: skill-md: no SKILL.md",
        "skills/gamma: symlink: the skill folder is a symlink",
    ] {
        assert!(said[1].contains(rule), "{rule}\n{}", said[1]);
    }
}

/// The same for a linked skill whose `SKILL.md` cannot be probed: its kept
/// link holds the name against a new canon skill until it can be.
#[test]
fn an_unprobed_skill_keeps_its_name_from_a_canon_skill() {
    let sandbox = company();
    let locked = sandbox.home().join("work/locked");
    write(locked.join("SKILL.md"), skill_md("alpha"));
    let top = sandbox.home().join("work/company/alpha/SKILL.md");
    std::fs::remove_file(&top).unwrap();
    std::os::unix::fs::symlink(locked.join("SKILL.md"), &top).unwrap();
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    std::fs::remove_file(claude.join("alpha")).unwrap();
    write(
        sandbox.canon().join("skills/alpha/SKILL.md"),
        skill_md("alpha"),
    );
    chmod(&locked, 0o000);

    let (code, _, stderr) = sync(&sandbox, &["sync"]);
    let vacant = exists(claude.join("alpha"));
    chmod(&locked, 0o755);

    assert_eq!(code, 3, "{stderr}");
    let line = "skill folder skills/alpha is not distributed: linked skill alpha of `company`, whose SKILL.md cannot be read now, keeps the name";
    assert!(stderr.contains(line), "{stderr}");
    assert!(!vacant, "{stderr}");
    assert_eq!(
        std::fs::read_link(sandbox.tool_dir(Tool::Codex).join("alpha")).unwrap(),
        sandbox.canon().join("linked/company/alpha")
    );

    assert_eq!(sync(&sandbox, &["sync"]).0, 3);
    for tool in Tool::ALL {
        assert_eq!(
            std::fs::read_link(sandbox.tool_dir(tool).join("alpha")).unwrap(),
            sandbox.canon().join("skills/alpha")
        );
    }
}

/// A path that cannot be checked because a folder on the way cannot be
/// entered is a read error of the whole folder, never a missing one.
#[test]
fn a_path_check_failing_with_a_permission_error_is_a_read_error() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    folder(&sandbox, "work/locked/company", &["alpha"]);
    append_manifest(
        &sandbox,
        "\n[linked.company]\npath = \"~/work/locked/company\"\n",
    );
    assert_eq!(sync(&sandbox, &["sync"]).0, 0);
    let locked = sandbox.home().join("work/locked");
    chmod(&locked, 0o000);

    let (code, _, stderr) = sync(&sandbox, &["sync"]);
    chmod(&locked, 0o755);

    assert_eq!(code, 3, "{stderr}");
    assert!(
        stderr.contains("linked folder `company` is not read ("),
        "{stderr}"
    );
    assert!(!stderr.contains("not on this machine"), "{stderr}");
    for tool in Tool::ALL {
        assert!(exists(sandbox.tool_dir(tool).join("alpha")));
    }
    assert!(exists(sandbox.canon().join("linked/company")));
}

/// A link an older script made into a folder that cannot be read: sync
/// names the folder, but not as one it distributes.
#[test]
fn a_link_into_a_folder_that_cannot_be_read_is_not_called_distributed() {
    let sandbox = company();
    let company = sandbox.home().join("work/company");
    let old = sandbox.tool_dir(Tool::ClaudeCode).join("old-alpha");
    std::os::unix::fs::symlink(company.join("alpha"), &old).unwrap();
    chmod(&company, 0o111);

    let (code, _, stderr) = sync(&sandbox, &["sync"]);
    chmod(&company, 0o755);

    assert_eq!(code, 3, "{stderr}");
    let line = format!(
        "warning: {} is a link into linked folder `company`\n",
        old.display()
    );
    assert!(stderr.contains(&line), "{line}\n{stderr}");
}

/// While a linked folder cannot be read, adopt refuses every entry: sync
/// and `init` name a foreign skill without suggesting `skiman adopt`.
#[test]
fn no_adopt_hint_while_a_linked_folder_cannot_be_read() {
    let sandbox = company();
    let company = sandbox.home().join("work/company");
    write(
        sandbox.tool_dir(Tool::ClaudeCode).join("other/SKILL.md"),
        skill_md("other"),
    );
    chmod(&company, 0o000);

    let (code, _, sync_stderr) = sync(&sandbox, &["sync"]);
    let init = sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.canon())
        .output()
        .unwrap();
    chmod(&company, 0o755);

    assert_eq!(code, 3, "{sync_stderr}");
    let init_stderr = String::from_utf8(init.stderr).unwrap();
    for stderr in [&sync_stderr, &init_stderr] {
        assert!(!stderr.contains("skiman adopt"), "{stderr}");
        assert!(!stderr.contains("skiman import"), "{stderr}");
        assert!(
            stderr.contains("once linked folder `company` can be read"),
            "{stderr}"
        );
    }
}

/// A single-skill folder that can be entered but not listed is a read
/// error of the whole id, like any other folder: its link stays (exit 3),
/// and no new canon is made while it lasts.
#[test]
fn a_single_skill_folder_that_cannot_be_listed_keeps_its_link() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    folder(&sandbox, "work", &["solo"]);
    append_manifest(&sandbox, "\n[linked.solo]\npath = \"~/work/solo\"\n");
    assert_eq!(sync(&sandbox, &["sync"]).0, 0);
    let solo = sandbox.home().join("work/solo");
    let second = sandbox.root().join("second");
    chmod(&solo, 0o111);

    let (code, _, stderr) = sync(&sandbox, &["sync"]);
    let init_second = sync(
        &sandbox,
        &["init", "--session-hooks", "none", second.to_str().unwrap()],
    );
    chmod(&solo, 0o755);

    assert_eq!(code, 3, "{stderr}");
    assert!(
        stderr.contains("linked folder `solo` is not read ("),
        "{stderr}"
    );
    let linked = sandbox.canon().join("linked/solo");
    for tool in Tool::ALL {
        assert_eq!(
            std::fs::read_link(sandbox.tool_dir(tool).join("solo")).unwrap(),
            linked
        );
    }
    assert_eq!(init_second.0, 1, "{}", init_second.2);
    assert!(!exists(&second));
}
