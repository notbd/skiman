//! Missing and unreadable linked folders (spec "Sync"): a folder that is
//! not on this machine gives its names up until it is back; one that
//! cannot be read keeps everything as it is, so a passing failure never
//! withdraws a skill or changes a collision winner.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::{append_manifest, folder};
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
    let manifest = sandbox.skill_repo().join("skiman.toml");
    std::fs::write(&manifest, read(&manifest).replace(from, to)).unwrap();
}

/// A canon with `~/work/company` (skills `alpha` and `beta`) registered and synced.
fn company() -> Sandbox {
    let sandbox = Sandbox::new();
    init(&sandbox);
    folder(&sandbox, "work/company", &["alpha", "beta"]);
    append_manifest(&sandbox, "\n[linked.company]\npath = \"~/work/company\"\n");
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
    assert!(!exists(sandbox.skill_repo().join("linked/company")));
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
        let linked = sandbox.skill_repo().join("linked");
        folder(&sandbox, "work/acme", &["alpha"]);
        append_manifest(&sandbox, "\n[linked.acme]\npath = \"~/work/acme\"\n");
        write(
            sandbox.skill_repo().join("skills/notes/SKILL.md"),
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
    assert!(exists(sandbox.skill_repo().join("linked/company")));
}
