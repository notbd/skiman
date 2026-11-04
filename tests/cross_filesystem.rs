//! Moves to a backup on another filesystem (spec Data formats, Machine
//! state): copy, verify, then delete the original. The state folder sits on
//! a small disk image (`hdiutil`, no sudo); where one cannot be attached, the
//! test prints why and skips.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use support::Sandbox;

/// A disk image attached at `mount`, detached on drop.
struct DiskImage {
    mount: PathBuf,
}

impl DiskImage {
    /// `Err` holds why no disk image could be attached.
    fn attach(sandbox: &Sandbox) -> Result<DiskImage, String> {
        let image = sandbox.root().join("state.dmg");
        let mount = sandbox.root().join("state-volume");
        std::fs::create_dir_all(&mount).map_err(|e| e.to_string())?;
        let run = |args: &[&str]| -> Result<(), String> {
            let out = Command::new("hdiutil")
                .args(args)
                .output()
                .map_err(|e| format!("cannot run hdiutil: {e}"))?;
            if out.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "hdiutil {} failed: {}",
                    args[0],
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            }
        };
        let image_arg = image.to_str().unwrap();
        run(&[
            "create",
            "-size",
            "8m",
            "-fs",
            "HFS+",
            "-volname",
            "skiman-state",
            "-layout",
            "NONE",
            image_arg,
        ])?;
        run(&[
            "attach",
            "-nobrowse",
            "-noverify",
            "-mountpoint",
            mount.to_str().unwrap(),
            image_arg,
        ])?;
        Ok(DiskImage { mount })
    }
}

impl Drop for DiskImage {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-force"])
            .arg(&self.mount)
            .output();
    }
}

fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

fn the_only_backup(state: &Path) -> PathBuf {
    let dirs: Vec<PathBuf> = std::fs::read_dir(state.join("skiman/backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs[0].clone()
}

#[test]
fn a_file_and_a_symlink_move_to_a_backup_on_another_filesystem() {
    let sandbox = Sandbox::new();
    let disk = match DiskImage::attach(&sandbox) {
        Ok(disk) => disk,
        Err(why) => {
            println!("skipped: no second filesystem: {why}");
            return;
        }
    };
    init(&sandbox);
    let tmp = sandbox.skill_repo().join(".skiman/tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("stray.md"), "left behind\n").unwrap();
    std::os::unix::fs::symlink("stray.md", tmp.join("link")).unwrap();
    let up = sandbox.upstream("acme/pdf");
    up.write(
        "SKILL.md",
        "---\nname: pdf\ndescription: Fill PDF forms\n---\n\n# pdf\n",
    );
    up.commit("v1");

    sandbox
        .skiman()
        .env("XDG_STATE_HOME", &disk.mount)
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);

    let saved = the_only_backup(&disk.mount).join("repo/.skiman/tmp");
    assert_eq!(
        std::fs::read_to_string(saved.join("stray.md")).unwrap(),
        "left behind\n"
    );
    assert!(
        std::fs::symlink_metadata(saved.join("link"))
            .unwrap()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_link(saved.join("link")).unwrap(),
        Path::new("stray.md")
    );
    assert!(std::fs::symlink_metadata(tmp.join("stray.md")).is_err());
    assert!(std::fs::symlink_metadata(tmp.join("link")).is_err());
}
