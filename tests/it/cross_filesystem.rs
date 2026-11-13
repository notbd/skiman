//! Writes across filesystems, each test with a folder on a second filesystem
//! (no sudo): a small disk image on macOS (`hdiutil`), `/dev/shm` on Linux.
//! A move to a backup on another filesystem (spec Data formats, Machine
//! state) copies, verifies, then deletes the original; and `project restore`
//! on a volume without hard links (exFAT, macOS only) fails without touching
//! the project file. Where there is no second filesystem, the test prints
//! why and skips, except in CI (`CI` is set), where it fails.

use std::os::unix::fs::MetadataExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::support::{Sandbox, Tool, init, moved_note, skill_md, tree, write};

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
            "error: cannot write {}: this volume cannot put a file in place without replacing what is there (",
            agents.display()
        )),
        "{stderr}"
    );
    // The closing sync does not send the user back to the restore that failed.
    assert!(!stderr.contains("project restore"), "{stderr}");
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

/// The kill matrix of a move to a backup on another filesystem: import
/// killed at each step of moving a foreign skill there, then run again.
/// Killed before the note is in place, the original is whole: sync reports
/// it as foreign and it is copied again (two complete copies). From the note
/// on it is no foreign skill: sync says nothing of it, doctor names its copy
/// (also when the name is one the import links), adopt refuses it naming its
/// copy, and the import run again deletes what is left and renames the one
/// copy into its own backup folder. Killed again while it does that (the rest
/// deleted, the copy renamed), the run after has nothing left to move.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_import_killed_at_each_step_of_a_move_to_another_filesystem_finishes_when_run_again() {
    let first = Sandbox::new();
    let Some(volume) = attached(second_filesystem(&first)) else {
        return;
    };
    // (kill point, kill point of the run after, the skill the import holds, copies)
    let cases = [
        ("copied", None, "notes", 2),
        ("noted", None, "notes", 1),
        ("deleting", None, "notes", 1),
        ("deleted", None, "notes", 1),
        ("noted", Some("rest-deleted"), "notes", 1),
        ("noted", Some("relocated"), "notes", 1),
        ("copied", None, "other", 2),
        ("noted", None, "other", 1),
        ("deleting", None, "other", 1),
    ];
    for (i, (point, again, imported, copies)) in cases.into_iter().enumerate() {
        let case = format!("{point} {again:?} {imported}");
        let sandbox = Sandbox::new();
        let state = volume.dir.join(i.to_string());
        let skiman = || sandbox.skiman().env("XDG_STATE_HOME", &state);
        let original = sandbox.tool_dir(Tool::ClaudeCode).join("notes");
        write(original.join("SKILL.md"), skill_md("notes"));
        for n in 0..20 {
            write(original.join(format!("refs/{n}.md")), format!("{n}\n"));
        }
        let before = tree(&original);
        skiman()
            .args(["init", "--session-hooks", "none"])
            .arg(sandbox.skill_repo())
            .assert()
            .code(0);
        let folder = sandbox.root().join("import");
        write(
            folder.join(format!("skills/{imported}/SKILL.md")),
            skill_md(imported),
        );
        write(folder.join("instructions/shared.md"), "# Shared\n");
        write(
            folder.join("skiman.toml"),
            "[tools]\nenabled = [\"claude-code\"]\n",
        );
        let import = || skiman().arg("import").arg(&folder).arg("--yes");
        let kill_at = |point: &str| {
            let killed = import()
                .env("SKIMAN_TEST_HOOK", format!("move:{point}"))
                .env("SKIMAN_TEST_HOOK_RUN", "kill -9 $PPID")
                .output()
                .unwrap();
            assert_eq!(killed.status.signal(), Some(9), "{case}: {killed:?}");
        };

        kill_at(point);
        let note = moved_note(&state, &original);
        assert_eq!(note.is_file(), point != "copied", "{case}");
        let sync = skiman().arg("sync").output().unwrap();
        let stderr = String::from_utf8_lossy(&sync.stderr);
        let foreign = imported != "notes" && point == "copied";
        assert_eq!(
            stderr.contains("foreign skill"),
            foreign,
            "{case}: {stderr}"
        );
        if matches!(point, "noted" | "deleting") {
            let copy = read_text(&note).lines().nth(1).unwrap().to_owned();
            let doctor = skiman().arg("doctor").output().unwrap();
            let said = String::from_utf8_lossy(&doctor.stdout);
            assert!(
                said.contains("cut short") && said.contains(&copy),
                "{case}: {said}"
            );
            if imported != "notes" {
                let adopt = skiman()
                    .args(["adopt", "notes", "--from", "claude-code", "--yes"])
                    .output()
                    .unwrap();
                let stderr = String::from_utf8_lossy(&adopt.stderr);
                assert_eq!(adopt.status.code(), Some(1), "{case}: {stderr}");
                assert!(
                    stderr.contains("cut short")
                        && stderr.contains(&copy)
                        && stderr.contains(
                            "to adopt the skill, delete it and move that copy here first"
                        ),
                    "{case}: {stderr}"
                );
            }
        }
        if let Some(again) = again {
            kill_at(again);
        }
        import().assert().code(0);

        let found: Vec<PathBuf> = std::fs::read_dir(state.join("skiman/backups"))
            .unwrap()
            .map(|e| e.unwrap().path().join("claude-code/notes"))
            .filter(|copy| copy.exists())
            .collect();
        assert_eq!(found.len(), copies, "{case}: {found:?}");
        for copy in &found {
            assert_eq!(tree(copy), before, "{case}");
        }
        let linked = std::fs::symlink_metadata(&original).is_ok_and(|m| m.is_symlink());
        assert_eq!(linked, imported == "notes", "{case}");
        assert!(
            std::fs::symlink_metadata(&original).is_ok() == linked,
            "{case}"
        );
        assert_eq!(
            note.exists(),
            point == "deleted" || again.is_some(),
            "{case}: only a note whose entry is gone stays"
        );
    }
}

/// How many files named `name` under the backups of `state` hold `text`.
fn copies_of(state: &Path, name: &str, text: &str) -> usize {
    let mut found = 0;
    let mut dirs = vec![state.join("skiman/backups")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                dirs.push(path);
            } else if entry.file_name() == name
                && std::fs::read_to_string(&path).is_ok_and(|t| t.contains(text))
            {
                found += 1;
            }
        }
    }
    found
}

/// The commands that move a whole folder's entries into a backup find what
/// a move to another filesystem cut short there: a project's swap folder
/// (`project restore`), a tmp leftover (`new`), an instruction file
/// (`import`). Killed at each step after the note, the next run asks for
/// nothing by hand, copies the old version no second time, and, while some
/// of it was left, says where the complete one is: in its own backup folder. A file has no `deleting`
/// step: one unlink deletes it.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_move_cut_short_in_a_swap_folder_tmp_or_an_instruction_location_is_finished_by_the_next_command()
 {
    let first = Sandbox::new();
    let Some(volume) = attached(second_filesystem(&first)) else {
        return;
    };
    let cases = [
        ("swap", "noted"),
        ("swap", "deleting"),
        ("swap", "deleted"),
        ("tmp", "noted"),
        ("tmp", "deleting"),
        ("tmp", "deleted"),
        ("instructions", "noted"),
        ("instructions", "deleted"),
    ];
    for (i, (place, point)) in cases.into_iter().enumerate() {
        let sandbox = Sandbox::new();
        let state = volume.dir.join(format!("c{i}"));
        let skiman = || sandbox.skiman().env("XDG_STATE_HOME", &state);
        std::fs::create_dir_all(sandbox.home().join(".claude")).unwrap();
        skiman()
            .args(["init", "--session-hooks", "none"])
            .arg(sandbox.skill_repo())
            .assert()
            .code(0);
        let project = sandbox.home().join("work");
        let skill = project.join(".claude/skills/foo");
        let tmp = sandbox
            .skill_repo()
            .join(".skiman/tmp/20200101T000000Z/old/skills/keep");
        let folder = sandbox.root().join("import");
        let (name, text) = match place {
            "swap" => {
                write(
                    skill.join("SKILL.md"),
                    format!("{}project original\n", skill_md("foo")),
                );
                for n in 0..4 {
                    write(skill.join(format!("f{n}.md")), format!("{n}\n"));
                }
                skiman()
                    .args(["project", "add"])
                    .arg(&project)
                    .assert()
                    .code(0);
                skiman()
                    .args(["project", "adopt", "--yes"])
                    .assert()
                    .code(0);
                let mirror = sandbox
                    .skill_repo()
                    .join("projects/work/.claude/skills/foo/SKILL.md");
                write(mirror, format!("{}mirror edit\n", skill_md("foo")));
                ("SKILL.md", "project original")
            }
            "tmp" => {
                write(tmp.join("SKILL.md"), "tmp leftover\n");
                for n in 0..4 {
                    write(tmp.join(format!("f{n}.md")), format!("{n}\n"));
                }
                ("SKILL.md", "tmp leftover")
            }
            _ => {
                std::fs::create_dir_all(folder.join("skills")).unwrap();
                // A file of the user's where init linked the shared body.
                std::fs::remove_file(sandbox.instruction_file(Tool::ClaudeCode)).unwrap();
                write(sandbox.instruction_file(Tool::ClaudeCode), "user rules\n");
                write(
                    folder.join("instructions/shared.md"),
                    read_text(&sandbox.skill_repo().join("instructions/shared.md")),
                );
                write(
                    folder.join("skiman.toml"),
                    read_text(&sandbox.skill_repo().join("skiman.toml")),
                );
                ("CLAUDE.md", "user rules")
            }
        };
        let run = || match place {
            "swap" => skiman().args(["project", "restore", "--yes"]).arg(&skill),
            "tmp" => skiman().args(["new", "other", "-d", "Use when testing."]),
            _ => skiman().arg("import").arg(&folder).arg("--yes"),
        };

        let killed = run()
            .env("SKIMAN_TEST_HOOK", format!("move:{point}"))
            .env("SKIMAN_TEST_HOOK_RUN", "kill -9 $PPID")
            .output()
            .unwrap();
        assert_eq!(
            killed.status.signal(),
            Some(9),
            "{place} {point}: {killed:?}"
        );
        if place == "swap" {
            // Something to write, so the restore looks at the swap folder.
            let mirror = sandbox
                .skill_repo()
                .join("projects/work/.claude/skills/foo/SKILL.md");
            write(mirror, format!("{}mirror edit 2\n", skill_md("foo")));
        }
        let again = run().output().unwrap();

        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&again.stdout),
            String::from_utf8_lossy(&again.stderr)
        );
        assert_eq!(again.status.code(), Some(0), "{place} {point}: {said}");
        assert!(!said.contains("by hand"), "{place} {point}: {said}");
        assert_eq!(copies_of(&state, name, text), 1, "{place} {point}: {said}");
        let empty = empty_folders(&state.join("skiman/backups"));
        assert!(empty.is_empty(), "{place} {point}: {empty:?}");
        // After `deleted` the old version is only in the backup: nothing to say.
        if point != "deleted" {
            let line = said
                .lines()
                .find(|l| l.contains("moved") && (place != "instructions" || l.contains(name)))
                .unwrap_or_else(|| panic!("{place} {point}: {said}"));
            let at = line.rsplit(" to ").next().unwrap();
            let held = match place {
                "tmp" => Path::new(at)
                    .join("20200101T000000Z/old/skills/keep")
                    .join(name),
                "swap" => Path::new(at).join("foo").join(name),
                _ => PathBuf::from(at),
            };
            assert!(read_text(&held).contains(text), "{place} {point}: {line}");
        }
    }
}

/// Every folder at or below `dir` that holds nothing, except below a tmp
/// leftover (moved as it was, empty staging folders and all).
fn empty_folders(dir: &Path) -> Vec<PathBuf> {
    if dir.ends_with("repo/.skiman/tmp") {
        return Vec::new();
    }
    let entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    match entries.is_empty() {
        true => vec![dir.to_owned()],
        false => entries
            .iter()
            .filter(|e| e.file_type().unwrap().is_dir())
            .flat_map(|e| empty_folders(&e.path()))
            .collect(),
    }
}

fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}
