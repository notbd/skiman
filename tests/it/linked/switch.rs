//! `init` switching to another canon: only skiman's own links into the
//! previous canon's `linked/` are cleaned up.

use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::Output;

use super::{COMPANY, append_manifest, folder, sync};
use crate::support::{Sandbox, Tool, init, write};

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
