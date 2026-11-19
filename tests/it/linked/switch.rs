//! `init` switching to another canon: the previous canon's registrations
//! still guard every write the switch makes outside the new canon, and
//! only skiman's own links into its `linked/` are cleaned up.

use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::Output;

use super::{COMPANY, append_manifest, contents, folder, sync};
use crate::support::{Sandbox, Tool, ToolSetup, init, read, write};

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

/// A registration of the canon `init` switches away from, re-pointed at
/// the folder holding a settings file while the session-hook preview waits
/// for its answer: the write reads that canon's registrations again under
/// the lock, so the entry is not written (exit 3).
#[test]
fn a_previous_registration_re_pointed_at_the_settings_file_while_init_asks_blocks_the_hook_entry() {
    let sandbox = Sandbox::builder()
        .tool(Tool::Antigravity, ToolSetup::Absent)
        .build();
    first(&sandbox);
    let company = folder(&sandbox, "company", &["beta"]);
    let vendor = sandbox.home().join("vendor");
    symlink(&company, &vendor).unwrap();
    append_manifest(&sandbox, "\n[linked.vendor]\npath = \"~/vendor\"\n");
    assert_eq!(sync(&sandbox).0, 0);
    let next = sandbox.home().join("next");
    write(next.join("settings.json"), "{}\n");
    let settings = sandbox.presence_dir(Tool::ClaudeCode).join("settings.json");
    symlink(next.join("settings.json"), &settings).unwrap();
    let second = sandbox.root().join("second");

    let mut pty = sandbox.skiman_pty([std::ffi::OsStr::new("init"), second.as_os_str()]);
    pty.expect("Add this entry? [y/N] ");
    std::fs::remove_file(&vendor).unwrap();
    symlink(&next, &vendor).unwrap();
    pty.send_line("y");
    let (code, rest) = pty.wait();

    assert_eq!(code, 3, "{rest}");
    assert!(
        rest.contains("inside linked folder `vendor`") && rest.contains("of the previous canon"),
        "{rest}"
    );
    assert_eq!(read(next.join("settings.json")), "{}\n");
}

/// The previous canon's `skiman.toml` broken right before `init` writes a
/// session-start hook entry: the guard cannot read that canon's
/// registrations, so it says so in the guard's words and leaves the
/// settings file alone (exit 3).
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_previous_canon_that_cannot_be_read_again_leaves_the_hook_entry_unwritten() {
    let sandbox = Sandbox::builder()
        .tool(Tool::Antigravity, ToolSetup::Absent)
        .build();
    first(&sandbox);
    let settings = sandbox.presence_dir(Tool::ClaudeCode).join("settings.json");
    let before = std::fs::read(&settings).ok();
    let manifest = sandbox.canon().join("skiman.toml");

    let output = sandbox
        .skiman()
        .args(["init", "--session-hooks", "claude-code"])
        .arg(sandbox.root().join("second"))
        .env("SKIMAN_TEST_HOOK", "session-hooks:guard")
        .env(
            "SKIMAN_TEST_HOOK_RUN",
            format!("printf '[[[\\n' >> '{}'", manifest.display()),
        )
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    let line = format!(
        "{} of the previous canon cannot be read (",
        manifest.display()
    );
    assert!(stderr.contains(&line), "{line}\n{stderr}");
    assert!(
        stderr.contains("; its session-start hook entry is left as it is"),
        "{stderr}"
    );
    assert_eq!(std::fs::read(&settings).ok(), before);
}
