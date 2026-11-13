//! Writes across filesystems, each test with a folder on a second filesystem
//! (no sudo): a small disk image on macOS (`hdiutil`), `/dev/shm` on Linux.
//! A move to a backup on another filesystem (spec Data formats, Machine
//! state) copies, verifies, then deletes the original; and `project restore`
//! on a volume without hard links (exFAT, macOS only) fails without touching
//! the project file. Where there is no second filesystem, the test prints
//! why and skips, except in CI (`CI` is set), where it fails.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::support::{Sandbox, init};

/// A disk image attached at `mount`, detached on drop.
struct DiskImage {
    mount: PathBuf,
}

impl DiskImage {
    /// A volume of filesystem `fs` (`hdiutil create -fs`), attached at
    /// `<sandbox>/<name>-volume`. `name` is also its label, which exFAT keeps
    /// to 11 characters. `Err` holds why no disk image could be attached.
    fn attach(sandbox: &Sandbox, fs: &str, name: &str) -> Result<DiskImage, String> {
        let image = sandbox.root().join(format!("{name}.dmg"));
        let mount = sandbox.root().join(format!("{name}-volume"));
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
            "create", "-size", "8m", "-fs", fs, "-volname", name, "-layout", "NONE", image_arg,
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

/// A folder on another filesystem than the sandbox's; what keeps it there
/// is dropped with it.
struct Volume {
    dir: PathBuf,
    _keep: Box<dyn std::any::Any>,
}

/// A volume with hard links and symlinks: an HFS+ disk image on macOS, a
/// folder in `/dev/shm` (a tmpfs) elsewhere.
fn second_filesystem(sandbox: &Sandbox) -> Result<Volume, String> {
    if cfg!(target_os = "macos") {
        let image = DiskImage::attach(sandbox, "HFS+", "state")?;
        return Ok(Volume {
            dir: image.mount.clone(),
            _keep: Box::new(image),
        });
    }
    let temp = tempfile::Builder::new()
        .prefix("skiman-test-")
        .tempdir_in("/dev/shm")
        .map_err(|e| format!("cannot create a folder in /dev/shm: {e}"))?;
    let device = |path: &Path| std::fs::metadata(path).map(|m| m.dev()).ok();
    if device(temp.path()) == device(sandbox.root()) {
        return Err("/dev/shm is on the sandbox's filesystem".to_owned());
    }
    Ok(Volume {
        dir: temp.path().to_owned(),
        _keep: Box::new(temp),
    })
}

/// The attached `volume`, or `None` after printing why there is none; in CI
/// that fails the test instead.
fn attached<T>(volume: Result<T, String>) -> Option<T> {
    match volume {
        Ok(volume) => Some(volume),
        Err(why) if std::env::var_os("CI").is_some() => {
            panic!("no second filesystem in CI: {why}")
        }
        #[allow(clippy::print_stdout)]
        Err(why) => {
            println!("skipped: no second filesystem: {why}");
            None
        }
    }
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
    let Some(state) = attached(second_filesystem(&sandbox)) else {
        return;
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
        .env("XDG_STATE_HOME", &state.dir)
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);

    let saved = the_only_backup(&state.dir).join("repo/.skiman/tmp");
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

#[cfg(target_os = "macos")]
#[test]
fn restore_over_a_file_on_a_volume_without_hard_links_fails_and_leaves_the_file_in_place() {
    let sandbox = Sandbox::new();
    let Some(disk) = attached(DiskImage::attach(&sandbox, "ExFAT", "project")) else {
        return;
    };
    init(&sandbox);
    let project = disk.mount.join("proj");
    let agents = project.join("AGENTS.md");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(&agents, "project original\n").unwrap();
    sandbox
        .skiman()
        .args(["project", "add"])
        .arg(&project)
        .assert()
        .code(0);
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    std::fs::write(
        sandbox.skill_repo().join("projects/proj/AGENTS.md"),
        "mirror edit\n",
    )
    .unwrap();

    let output = sandbox
        .skiman()
        .args(["project", "restore", "--yes"])
        .arg(&agents)
        .assert()
        .code(4)
        .get_output()
        .clone();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        std::fs::read_to_string(&agents).ok().as_deref(),
        Some("project original\n"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "error: cannot write {}: the hard-link probe failed (",
            agents.display()
        )),
        "{stderr}"
    );
    // exFAT adds `._*` files of its own, so only skiman's swap folders count.
    let swaps: Vec<_> = std::fs::read_dir(&project)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|name| name.to_string_lossy().starts_with(".skiman-swap-"))
        .collect();
    assert!(swaps.is_empty(), "{swaps:?}");
}

#[test]
fn restore_moves_the_replaced_version_into_a_backup_on_another_filesystem() {
    let sandbox = Sandbox::new();
    let Some(state) = attached(second_filesystem(&sandbox)) else {
        return;
    };
    init(&sandbox);
    let skiman = |args: &[&str]| {
        sandbox
            .skiman()
            .env("XDG_STATE_HOME", &state.dir)
            .args(args)
    };
    let project = sandbox.home().join("work");
    let agents = project.join("AGENTS.md");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(&agents, "project original\n").unwrap();
    skiman(&["project", "add"]).arg(&project).assert().code(0);
    skiman(&["project", "adopt", "--yes"]).assert().code(0);
    std::fs::write(
        sandbox.skill_repo().join("projects/work/AGENTS.md"),
        "mirror edit\n",
    )
    .unwrap();

    skiman(&["project", "restore", "--yes"])
        .arg(&agents)
        .assert()
        .code(0);

    assert_eq!(std::fs::read_to_string(&agents).unwrap(), "mirror edit\n");
    let saved = the_only_backup(&state.dir).join("projects/work/AGENTS.md");
    assert_eq!(
        std::fs::read_to_string(saved).unwrap(),
        "project original\n"
    );
    let swaps: Vec<_> = std::fs::read_dir(&project)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|name| name.to_string_lossy().starts_with(".skiman-swap-"))
        .collect();
    assert!(swaps.is_empty(), "{swaps:?}");
}
