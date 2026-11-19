//! `init` switching to another canon: the previous canon's registrations
//! still guard every write the switch makes outside the new canon, and
//! only skiman's own links into its `linked/` are cleaned up.

use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::Output;

use super::{COMPANY, append_manifest, contents, folder, sync};
use crate::support::{Sandbox, Tool, init, read, write};

/// A canon at `sandbox.canon()` with `work/company` (skill `alpha`)
/// registered and synced; returns the folder.
fn first(sandbox: &Sandbox) -> PathBuf {
    init(sandbox);
    let company = folder(sandbox, "work/company", &["alpha"]);
    append_manifest(sandbox, COMPANY);
    assert_eq!(sync(sandbox).0, 0);
    company
}

fn init_at(sandbox: &Sandbox, path: &std::path::Path, hooks: &str) -> Output {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", hooks])
        .arg(path)
        .output()
        .unwrap()
}

/// A tool directory turned into a link into a folder registered in the
/// canon `init` switches away from: the closing sync writes nothing there
/// (exit 3), though the new canon does not register it.
#[test]
fn a_tool_directory_aliased_into_the_previous_canons_folder_is_left_alone() {
    let sandbox = Sandbox::new();
    let company = first(&sandbox);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    std::fs::remove_dir_all(&claude).unwrap();
    symlink(&company, &claude).unwrap();
    let before = contents(&[&company]);

    let output = init_at(&sandbox, &sandbox.root().join("second"), "none");

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("inside linked folder `company`")
            && stderr.contains("of the previous canon"),
        "{stderr}"
    );
    assert_eq!(contents(&[&company]), before);
}

/// A reused canon whose `.gitignore` is a link into a folder registered in
/// the previous canon: `init` adds nothing to it (exit 3).
#[test]
fn a_reused_canons_gitignore_linked_into_the_previous_canons_folder_is_not_written() {
    let sandbox = Sandbox::new();
    let company = first(&sandbox);
    let second = sandbox.root().join("second");
    std::fs::create_dir_all(&second).unwrap();
    std::fs::write(second.join("skiman.toml"), "[tools]\nenabled = []\n").unwrap();
    sandbox.git(&second, ["init", "--quiet"]);
    write(company.join("ignore"), "target\n");
    symlink(company.join("ignore"), second.join(".gitignore")).unwrap();
    let before = contents(&[&company]);

    let output = init_at(&sandbox, &second, "none");

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("inside linked folder `company`")
            && stderr.contains("`.skiman/` is not added to it"),
        "{stderr}"
    );
    assert_eq!(contents(&[&company]), before);
}

/// A settings file that is a link into a folder registered in the previous
/// canon: `init` writes no session-start hook entry there (exit 3).
#[test]
fn a_settings_file_linked_into_the_previous_canons_folder_gets_no_hook_entry() {
    let sandbox = Sandbox::new();
    let company = first(&sandbox);
    write(company.join("settings.json"), "{}\n");
    let settings = sandbox.presence_dir(Tool::ClaudeCode).join("settings.json");
    symlink(company.join("settings.json"), &settings).unwrap();
    let before = contents(&[&company]);

    let output = init_at(&sandbox, &sandbox.root().join("second"), "claude-code");

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("its session-start hook entry is left as it is"),
        "{stderr}"
    );
    assert_eq!(read(company.join("settings.json")), "{}\n");
    assert_eq!(contents(&[&company]), before);
}

/// Switching canons cleans up skiman's own links into the previous
/// canon's `linked/`, which have the two exact shapes, and leaves a link
/// of any other shape there alone, as a sync of that canon would.
#[test]
fn a_switch_removes_only_skimans_own_links_into_the_previous_canons_linked_folder() {
    let sandbox = Sandbox::new();
    first(&sandbox);
    let linked = sandbox.canon().join("linked");
    folder(&sandbox, "work/solo", &[]);
    write(
        sandbox.home().join("work/solo/SKILL.md"),
        crate::support::skill_md("solo"),
    );
    append_manifest(&sandbox, "\n[linked.solo]\npath = \"~/work/solo\"\n");
    assert_eq!(sync(&sandbox).0, 0);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    symlink(linked.join("company/alpha"), claude.join("alias")).unwrap();
    symlink(linked.join("solo"), claude.join("other")).unwrap();

    let output = init_at(&sandbox, &sandbox.root().join("second"), "none");

    let stderr = String::from_utf8(output.stderr).unwrap();
    for name in ["alpha", "solo"] {
        assert!(
            std::fs::symlink_metadata(claude.join(name)).is_err(),
            "{name}\n{stderr}"
        );
    }
    assert_eq!(
        std::fs::read_link(claude.join("alias")).unwrap(),
        linked.join("company/alpha")
    );
    assert_eq!(
        std::fs::read_link(claude.join("other")).unwrap(),
        linked.join("solo")
    );
}
