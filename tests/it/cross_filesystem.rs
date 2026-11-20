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
    let tmp = sandbox.canon().join(".skiman/tmp");
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
        sandbox.canon().join("projects/proj/AGENTS.md"),
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

/// `project adopt` that cannot write the mirror (a canon on exFAT) fails
/// with exit 4, leaves the mirror as it was, and the closing sync does not
/// send the user back to the adopt that failed. The mirror is put there by
/// hand: adopt itself cannot write it on this volume.
#[cfg(target_os = "macos")]
#[test]
fn adopt_into_a_canon_on_a_volume_without_hard_links_fails_and_leaves_the_mirror() {
    let sandbox = Sandbox::new();
    let Some(disk) = attached(DiskImage::attach(&sandbox, "ExFAT", "canon")) else {
        return;
    };
    let canon = disk.mount.join("canon");
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&canon)
        .assert()
        .code(0);
    let project = sandbox.home().join("work/proj");
    let agents = project.join("AGENTS.md");
    write(&agents, "v1\n");
    sandbox
        .skiman()
        .args(["project", "add", "--id", "work"])
        .arg(project.parent().unwrap())
        .assert()
        .code(0);
    let mirror = canon.join("projects/work/proj/AGENTS.md");
    write(&mirror, "v1\n");
    sandbox.skiman().arg("sync").assert().code(0);
    std::fs::write(&agents, "project edit\n").unwrap();
    std::fs::write(&mirror, "mirror edit\n").unwrap();
    let conflict = sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(3)
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("skiman project adopt"));

    let output = sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .arg(&agents)
        .assert()
        .code(4)
        .get_output()
        .clone();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        std::fs::read_to_string(&mirror).unwrap(),
        "mirror edit\n",
        "{stderr}"
    );
    assert!(
        stderr.contains("this volume cannot put a file in place without replacing what is there ("),
        "{stderr}"
    );
    // The closing sync does not send the user back to the adopt that failed.
    assert!(!stderr.contains("skiman project adopt"), "{stderr}");
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
        sandbox.canon().join("projects/work/AGENTS.md"),
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
            .arg(sandbox.canon())
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
                let then = format!(
                    "; delete it; then, to adopt the skill, move that copy to {} and run the command again",
                    original.display()
                );
                assert!(
                    stderr.contains("cut short")
                        && stderr.contains(&copy)
                        && stderr.contains(&then),
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
            .arg(sandbox.canon())
            .assert()
            .code(0);
        let project = sandbox.home().join("work");
        let skill = project.join(".claude/skills/foo");
        let tmp = sandbox
            .canon()
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
                    .canon()
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
                    read_text(&sandbox.canon().join("instructions/shared.md")),
                );
                write(
                    folder.join("skiman.toml"),
                    read_text(&sandbox.canon().join("skiman.toml")),
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
                .canon()
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
            // A cut-short move in a staging folder leaves its copy where it is.
            let said_line = match place {
                "instructions" => "moved",
                _ => "deleted what was left of",
            };
            let line = said
                .lines()
                .find(|l| l.contains(said_line) && (place != "instructions" || l.contains(name)))
                .unwrap_or_else(|| panic!("{place} {point}: {said}"));
            let at = line.rsplit(" to ").next().unwrap();
            let held = match place {
                "tmp" => Path::new(at).join("old/skills/keep").join(name),
                "swap" => Path::new(at).join(name),
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

/// On a filesystem without a swapping rename (HFS+), update renames the old
/// `skills/<name>` into `.skiman/tmp/` and the merge in, then moves the old
/// one to the backup, which, on another filesystem, copies and deletes.
/// Killed while it deletes, the skill itself is already the merge: the
/// update run again finishes, every local file in the skill, and the old
/// local files are in a backup.
#[cfg(target_os = "macos")]
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_update_killed_while_the_old_skill_moves_to_another_filesystem_finishes_when_run_again() {
    let sandbox = Sandbox::new();
    let Some((_disk, repo)) = pdf_on_a_volume_without_a_swap(&sandbox) else {
        return;
    };

    let killed = sandbox
        .skiman()
        .args(["update", "pdf"])
        .env("SKIMAN_TEST_HOOK", "move:deleting")
        .env("SKIMAN_TEST_HOOK_RUN", "kill -9 $PPID")
        .output()
        .unwrap();
    assert_eq!(killed.status.signal(), Some(9));
    sandbox.skiman().args(["update", "pdf"]).assert().code(0);

    let skill = repo.join("skills/pdf");
    assert_eq!(read_text(&skill.join("reference.md")), "v2\n");
    for n in 0..10 {
        assert_eq!(
            read_text(&skill.join(format!("refs/{n}.md"))),
            format!("{n}\n")
        );
    }
    let state = sandbox.state_home();
    assert!(
        copies_of(&state, "9.md", "9\n") >= 1,
        "the old refs are in a backup"
    );
    sandbox.skiman().arg("check").assert().code(0);
    assert_eq!(
        tmp_entries(&repo),
        Vec::<PathBuf>::new(),
        "nothing is left in tmp"
    );
}

/// A third-party pdf with ten local files in a canon on an HFS+ volume
/// (no swapping rename), and an upstream fix to update to.
#[cfg(target_os = "macos")]
fn pdf_on_a_volume_without_a_swap(sandbox: &Sandbox) -> Option<(DiskImage, PathBuf)> {
    pdf_on_a_volume_without_a_swap_at(sandbox, "skills/pdf")
}

/// [`pdf_on_a_volume_without_a_swap`] with the skill folder at `skill`
/// (`skills/pdf` or `skills/<group>/pdf`).
#[cfg(target_os = "macos")]
fn pdf_on_a_volume_without_a_swap_at(
    sandbox: &Sandbox,
    skill: &str,
) -> Option<(DiskImage, PathBuf)> {
    let disk = attached(DiskImage::attach(sandbox, "HFS+", "repo"))?;
    let repo = disk.mount.join("canon");
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&repo)
        .assert()
        .code(0);
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", skill_md("pdf"))
        .write("reference.md", "v1\n");
    up.commit("v1");
    let mut args = vec!["add", "acme/pdf", "--yes"];
    if let Some(group) = skill
        .strip_prefix("skills/")
        .and_then(|s| s.strip_suffix("/pdf"))
    {
        args.extend(["--group", group]);
    }
    sandbox.skiman().args(args).assert().code(0);
    for n in 0..10 {
        write(repo.join(format!("{skill}/refs/{n}.md")), format!("{n}\n"));
    }
    up.write("reference.md", "v2\n");
    up.commit("v2");
    Some((disk, repo))
}

/// What `.skiman/tmp/` of `repo` holds.
#[cfg(target_os = "macos")]
fn tmp_entries(repo: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(repo.join(".skiman/tmp")) {
        Ok(entries) => entries.map(|e| e.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

/// The one `tmp/<ts>/old` folder a stopped replace left in `repo`.
#[cfg(target_os = "macos")]
fn the_only_old(repo: &Path) -> PathBuf {
    let entries = tmp_entries(repo);
    assert_eq!(entries.len(), 1, "{entries:?}");
    entries[0].join("old")
}

/// Asserts that `old`, the `tmp/<ts>/old` of a replace of the skill folder
/// `skill` stopped between its two renames, holds the whole old skill
/// (`before`) at `<skill>.skiman-prev` and the whole new one (the upstream
/// fix merged in) at `<skill>`.
#[cfg(target_os = "macos")]
fn assert_both_versions(
    old: &Path,
    skill: &str,
    before: &std::collections::BTreeMap<PathBuf, String>,
) {
    let mut new = before.clone();
    let reference = new.get_mut(Path::new("reference.md")).unwrap();
    *reference = reference.replace("v1\n", "v2\n");
    assert_eq!(&tree(old.join(format!("{skill}.skiman-prev"))), before);
    assert_eq!(tree(old.join(skill)), new);
}

/// Killed between the two renames of a replace without a swapping rename,
/// `skills/<name>` is missing and both versions are in `.skiman/tmp/`: the
/// old one at `old/skills/<name>.skiman-prev`, the new one at `old/skills/<name>`.
/// Run again, the update puts the new one in place and finishes.
#[cfg(target_os = "macos")]
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_update_killed_between_the_two_renames_keeps_both_versions_in_tmp() {
    let sandbox = Sandbox::new();
    let Some((_disk, repo)) = pdf_on_a_volume_without_a_swap(&sandbox) else {
        return;
    };
    let skill = repo.join("skills/pdf");
    let before = tree(&skill);
    let killed = sandbox
        .skiman()
        .args(["update", "pdf"])
        .env("SKIMAN_TEST_HOOK", "update:set-aside:skills/pdf")
        .env("SKIMAN_TEST_HOOK_RUN", "kill -9 $PPID")
        .output()
        .unwrap();
    assert_eq!(killed.status.signal(), Some(9));

    assert!(!skill.exists());
    assert_both_versions(&the_only_old(&repo), "skills/pdf", &before);
    // A dry run writes nothing: the skill stays missing and tmp as it is.
    let dry = sandbox
        .skiman()
        .args(["update", "pdf", "--dry-run"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&dry.stderr);
    assert_eq!(dry.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("skills/pdf is not a folder"), "{stderr}");
    assert!(!skill.exists());
    assert_both_versions(&the_only_old(&repo), "skills/pdf", &before);
    let new = the_only_old(&repo).join("skills/pdf");
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "put {} in place from {}, where a replace stopped part-way left it\n...\n",
            skill.display(),
            new.display()
        ));
    assert_updated(&sandbox, &repo, "skills/pdf");
}

/// The same in a nested group folder: killed between the two renames, the
/// old one is at `old/skills/<group>/<name>.skiman-prev` and the empty group
/// folders stay; run again, the update puts the new one back in the group
/// folder, not at `skills/<name>`.
#[cfg(target_os = "macos")]
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_update_of_a_grouped_skill_killed_between_the_two_renames_finishes_in_its_group() {
    let sandbox = Sandbox::new();
    let rel = "skills/documents/forms/pdf";
    let Some((_disk, repo)) = pdf_on_a_volume_without_a_swap_at(&sandbox, rel) else {
        return;
    };
    let skill = repo.join(rel);
    let before = tree(&skill);
    let killed = sandbox
        .skiman()
        .args(["update", "pdf"])
        .env("SKIMAN_TEST_HOOK", format!("update:set-aside:{rel}"))
        .env("SKIMAN_TEST_HOOK_RUN", "kill -9 $PPID")
        .output()
        .unwrap();
    assert_eq!(killed.status.signal(), Some(9));
    assert!(!skill.exists() && repo.join("skills/documents/forms").is_dir());
    assert_both_versions(&the_only_old(&repo), rel, &before);

    let new = the_only_old(&repo).join(rel);
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "put {} in place from {}, where a replace stopped part-way left it\n...\n",
            skill.display(),
            new.display()
        ));
    assert!(!repo.join("skills/pdf").exists());
    assert_updated(&sandbox, &repo, rel);
}

/// The skill folder `skill` holds the fix the update brings, and nothing is
/// left over.
#[cfg(target_os = "macos")]
fn assert_updated(sandbox: &Sandbox, repo: &Path, skill: &str) {
    let skill = repo.join(skill);
    assert_eq!(read_text(&skill.join("reference.md")), "v2\n");
    for n in 0..10 {
        let file = skill.join(format!("refs/{n}.md"));
        assert_eq!(read_text(&file), format!("{n}\n"));
    }
    sandbox.skiman().arg("check").assert().code(0);
    assert_eq!(tmp_entries(repo), Vec::<PathBuf>::new());
}

/// When the second rename of such a replace fails, the update stops with
/// exit 4 and says where the old and the new version are; run again, it
/// finishes.
#[cfg(target_os = "macos")]
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_failed_second_rename_says_where_both_versions_are() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new();
    let Some((_disk, repo)) = pdf_on_a_volume_without_a_swap(&sandbox) else {
        return;
    };
    let (skills, skill) = (repo.join("skills"), repo.join("skills/pdf"));
    let before = tree(&skill);
    let output = sandbox
        .skiman()
        .args(["update", "pdf"])
        .env("SKIMAN_TEST_HOOK", "update:set-aside:skills/pdf")
        .env(
            "SKIMAN_TEST_HOOK_RUN",
            format!("chmod 555 '{}'", skills.display()),
        )
        .output()
        .unwrap();
    std::fs::set_permissions(&skills, std::fs::Permissions::from_mode(0o755)).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(4), "{stderr}");

    let old = the_only_old(&repo);
    let (prev, new) = (old.join("skills/pdf.skiman-prev"), old.join("skills/pdf"));
    let place = format!(
        "; the old {skill} is at {} and the new one at {}\n",
        prev.display(),
        new.display(),
        skill = skill.display(),
    );
    let stopped = "the command stopped part-way: the lines above list what was done; run the same command again to finish it\n";
    assert!(stderr.contains(&format!("{place}{stopped}")), "{stderr}");
    assert_both_versions(&old, "skills/pdf", &before);

    sandbox.skiman().args(["update", "pdf"]).assert().code(0);
    assert!(!new.exists());
    assert_updated(&sandbox, &repo, "skills/pdf");
}

/// An adopt killed while the original moves to a backup on another
/// filesystem finishes when run again: `skills/<name>` already holds the
/// copy, and what the cut-short move left is read from its complete copy
/// and finished. Killed before the note (`copied`), the original is whole
/// and is copied once more.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_adopt_killed_while_the_original_moves_to_another_filesystem_finishes_when_run_again() {
    let first = Sandbox::new();
    let Some(volume) = attached(second_filesystem(&first)) else {
        return;
    };
    let args = ["adopt", "notes", "--from", "claude-code", "--yes"];
    for (i, (point, copies)) in [("copied", 2), ("noted", 1), ("deleting", 1)]
        .into_iter()
        .enumerate()
    {
        let sandbox = Sandbox::new();
        let state = volume.dir.join(i.to_string());
        let skiman = || sandbox.skiman().env("XDG_STATE_HOME", &state);
        skiman()
            .args(["init", "--session-hooks", "none"])
            .arg(sandbox.canon())
            .assert()
            .code(0);
        let original = sandbox.tool_dir(Tool::ClaudeCode).join("notes");
        write(original.join("SKILL.md"), skill_md("notes"));
        for n in 0..5 {
            write(original.join(format!("refs/{n}.md")), format!("{n}\n"));
        }

        let killed = skiman()
            .args(args)
            .env("SKIMAN_TEST_HOOK", format!("move:{point}"))
            .env("SKIMAN_TEST_HOOK_RUN", "kill -9 $PPID")
            .output()
            .unwrap();
        assert_eq!(killed.status.signal(), Some(9), "{point}");
        let again = skiman().args(args).output().unwrap();
        assert_eq!(
            again.status.code(),
            Some(0),
            "{point}: {}",
            String::from_utf8_lossy(&again.stderr)
        );

        let skill = sandbox.canon().join("skills/notes");
        assert_eq!(std::fs::read_link(&original).unwrap(), skill, "{point}");
        assert_eq!(copies_of(&state, "4.md", "4\n"), copies, "{point}");
        let check = skiman().arg("check").output().unwrap();
        assert_eq!(check.status.code(), Some(0), "{point}");
    }
}

/// On a volume without a swapping rename, a replace takes the old folder
/// out first and checks it there. Edited as it was taken out, it goes back
/// with the edit and the new one stays in `.skiman/tmp/`; run again, the
/// update keeps the edit as a local change.
#[cfg(target_os = "macos")]
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_folder_edited_as_it_was_taken_out_goes_back() {
    let sandbox = Sandbox::new();
    let Some((_disk, repo)) = pdf_on_a_volume_without_a_swap(&sandbox) else {
        return;
    };
    let skill = repo.join("skills/pdf");
    let prev = format!(
        "{}/.skiman/tmp/*/old/skills/pdf.skiman-prev",
        repo.display()
    );
    let edit = format!("printf 'edited\\n' >> $(ls -d {prev})/refs/0.md");
    let output = sandbox
        .skiman()
        .args(["update", "pdf"])
        .env("SKIMAN_TEST_HOOK", "update:set-aside:skills/pdf")
        .env("SKIMAN_TEST_HOOK_RUN", edit)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(4), "{stderr}");
    let new = the_only_old(&repo).join("skills/pdf");
    let changed = format!(
        "{} changed while skiman was writing; it is back where it was, and what came out is at {}",
        skill.display(),
        new.display()
    );
    assert!(stderr.contains(&changed), "{stderr}");
    assert_eq!(read_text(&skill.join("refs/0.md")), "0\nedited\n");
    assert_eq!(read_text(&skill.join("reference.md")), "v1\n");
    assert_eq!(read_text(&new.join("reference.md")), "v2\n");

    sandbox.skiman().args(["update", "pdf"]).assert().code(0);
    assert_eq!(read_text(&skill.join("reference.md")), "v2\n");
    assert_eq!(read_text(&skill.join("refs/0.md")), "0\nedited\n");
}

/// When something also took the folder's place, the edited folder cannot go
/// back: both stay where they are, and the error says where.
#[cfg(target_os = "macos")]
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_folder_edited_as_it_was_taken_out_whose_place_was_taken_stays_where_it_is() {
    let sandbox = Sandbox::new();
    let Some((_disk, repo)) = pdf_on_a_volume_without_a_swap(&sandbox) else {
        return;
    };
    let skill = repo.join("skills/pdf");
    let prev = format!(
        "{}/.skiman/tmp/*/old/skills/pdf.skiman-prev",
        repo.display()
    );
    let edit = format!(
        "printf 'edited\\n' >> $(ls -d {prev})/refs/0.md && printf mine > '{}'",
        skill.display()
    );
    let output = sandbox
        .skiman()
        .args(["update", "pdf"])
        .env("SKIMAN_TEST_HOOK", "update:set-aside:skills/pdf")
        .env("SKIMAN_TEST_HOOK_RUN", edit)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(4), "{stderr}");
    let old = the_only_old(&repo);
    let (prev, new) = (old.join("skills/pdf.skiman-prev"), old.join("skills/pdf"));
    let said = format!(
        "{} changed while skiman was writing and cannot be put back (File exists (os error 17)): it is at {}, and the new one at {}",
        skill.display(),
        prev.display(),
        new.display()
    );
    assert!(stderr.contains(&said), "{stderr}");
    assert_eq!(read_text(&skill), "mine");
    assert_eq!(read_text(&prev.join("refs/0.md")), "0\nedited\n");
    assert_eq!(read_text(&new.join("reference.md")), "v2\n");
}

/// The same check for a project item: restore on a volume without a
/// swapping rename puts back a project file edited as it was taken out,
/// keeps the new one in the staging folder and reports a conflict.
#[cfg(target_os = "macos")]
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_project_file_edited_as_restore_took_it_out_goes_back() {
    let sandbox = Sandbox::new();
    let Some(disk) = attached(DiskImage::attach(&sandbox, "HFS+", "proj")) else {
        return;
    };
    init(&sandbox);
    let project = disk.mount.join("proj");
    let agents = project.join("AGENTS.md");
    write(&agents, "project original\n");
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
    write(
        sandbox.canon().join("projects/proj/AGENTS.md"),
        "mirror edit\n",
    );
    let prev = format!(
        "{}/.skiman-swap-*/old/AGENTS.md.skiman-prev",
        project.display()
    );

    let output = sandbox
        .skiman()
        .args(["project", "restore", "--yes"])
        .arg(&agents)
        .env("SKIMAN_TEST_HOOK", "project:set-aside:AGENTS.md")
        .env(
            "SKIMAN_TEST_HOOK_RUN",
            format!("printf 'edited\\n' >> $(ls {prev})"),
        )
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    let swaps: Vec<_> = std::fs::read_dir(&project)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".skiman-swap-")
        })
        .collect();
    let [swap] = swaps.as_slice() else {
        panic!("{swaps:?}")
    };
    let new = swap.join("old/AGENTS.md");
    let said = format!(
        "warning: {} changed since the plan was made (what came out is at {}); nothing was replaced, run the command again",
        agents.display(),
        new.display()
    );
    assert!(stderr.contains(&said), "{stderr}");
    assert_eq!(read_text(&agents), "project original\nedited\n");
    assert_eq!(read_text(&new), "mirror edit\n");
}

/// Killed between the two renames of a restore on a volume without a
/// swapping rename, the project file is missing and both versions are in the
/// staging folder: run again, the restore moves both to the backup with
/// the tmp leftover and writes the item from the mirror.
#[cfg(target_os = "macos")]
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_restore_killed_between_its_two_renames_finishes_when_run_again() {
    let sandbox = Sandbox::new();
    let Some(disk) = attached(DiskImage::attach(&sandbox, "HFS+", "proj")) else {
        return;
    };
    init(&sandbox);
    let project = disk.mount.join("proj");
    let agents = project.join("AGENTS.md");
    write(&agents, "project original\n");
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
    write(
        sandbox.canon().join("projects/proj/AGENTS.md"),
        "mirror edit\n",
    );
    let restore = || {
        sandbox
            .skiman()
            .args(["project", "restore", "--yes"])
            .arg(&agents)
    };
    let killed = restore()
        .env("SKIMAN_TEST_HOOK", "project:set-aside:AGENTS.md")
        .env("SKIMAN_TEST_HOOK_RUN", "kill -9 $PPID")
        .output()
        .unwrap();
    assert_eq!(killed.status.signal(), Some(9));
    assert!(!agents.exists());

    let again = restore().output().unwrap();
    let said = String::from_utf8_lossy(&again.stdout);
    assert_eq!(
        again.status.code(),
        Some(0),
        "{said}{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert!(said.contains("moved tmp leftovers to "), "{said}");
    assert_eq!(read_text(&agents), "mirror edit\n");
    // The original went to the backup with the tmp leftover it was in.
    let backups = tree(sandbox.state_home().join("skiman/backups"));
    let original = backups
        .values()
        .filter(|e| *e == "file 644 project original\n");
    assert_eq!(original.count(), 1, "{backups:?}");
}

/// Killed between the two renames on a volume without a swapping rename,
/// then the source edited (the project file for `adopt`, its mirror for
/// `restore`): run again, the command writes the edited source, not what
/// the stopped run had staged.
#[cfg(target_os = "macos")]
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_project_write_killed_between_its_two_renames_writes_the_source_as_it_is_now() {
    for verb in ["adopt", "restore"] {
        let sandbox = Sandbox::new();
        let Some(disk) = attached(DiskImage::attach(&sandbox, "HFS+", "vol")) else {
            return;
        };
        let repo = disk.mount.join("canon");
        sandbox
            .skiman()
            .args(["init", "--session-hooks", "none"])
            .arg(&repo)
            .assert()
            .code(0);
        let project = disk.mount.join("proj");
        let (agents, mirror) = (
            project.join("AGENTS.md"),
            repo.join("projects/proj/AGENTS.md"),
        );
        write(&agents, "project original\n");
        let skiman = |args: &[&str]| sandbox.skiman().args(args).output().unwrap();
        assert_eq!(
            skiman(&["project", "add", &project.to_string_lossy()])
                .status
                .code(),
            Some(0)
        );
        assert_eq!(
            skiman(&["project", "adopt", "--yes"]).status.code(),
            Some(0)
        );
        // What the stopped run stages, then the edit after it was killed.
        let (source, point) = match verb {
            "adopt" => (&agents, "project:set-aside:projects/proj/AGENTS.md"),
            _ => (&mirror, "project:set-aside:AGENTS.md"),
        };
        write(source, "staged\n");
        let run = || {
            sandbox
                .skiman()
                .args(["project", verb, "--yes"])
                .arg(&agents)
        };
        let killed = run()
            .env("SKIMAN_TEST_HOOK", point)
            .env("SKIMAN_TEST_HOOK_RUN", "kill -9 $PPID")
            .output()
            .unwrap();
        assert_eq!(killed.status.signal(), Some(9), "{verb}");
        write(source, "edited after the kill\n");

        let again = run().output().unwrap();
        let stderr = String::from_utf8_lossy(&again.stderr);
        assert_eq!(again.status.code(), Some(0), "{verb}: {stderr}");
        assert_eq!(read_text(&agents), "edited after the kill\n", "{verb}");
        assert_eq!(read_text(&mirror), "edited after the kill\n", "{verb}");
        assert_eq!(skiman(&["sync"]).status.code(), Some(0), "{verb}");
    }
}

/// A replaced project file moved to a backup on another filesystem, whose
/// rest cannot be deleted once its complete copy is there: the error says
/// the move did not finish and what is left to delete, not that the
/// previous version is kept there.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_restore_whose_previous_version_is_copied_but_not_deleted_says_what_to_delete() {
    let sandbox = Sandbox::new();
    let Some(state) = attached(second_filesystem(&sandbox)) else {
        return;
    };
    init(&sandbox);
    let skiman = || sandbox.skiman().env("XDG_STATE_HOME", &state.dir);
    let project = sandbox.home().join("work");
    let agents = project.join("AGENTS.md");
    write(&agents, "project original\n");
    skiman()
        .args(["project", "add"])
        .arg(&project)
        .assert()
        .code(0);
    skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    write(
        sandbox.canon().join("projects/work/AGENTS.md"),
        "mirror edit\n",
    );
    let old = format!("{}/.skiman-swap-*/old", project.display());

    let output = skiman()
        .args(["project", "restore", "--yes"])
        .arg(&agents)
        .env("SKIMAN_TEST_HOOK", "move:noted")
        .env("SKIMAN_TEST_HOOK_RUN", format!("chmod 555 {old}"))
        .output()
        .unwrap();
    let left = std::fs::read_dir(&project)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".skiman-swap-")
        })
        .unwrap()
        .join("old");
    let unlocked = Command::new("chmod")
        .arg("755")
        .arg(&left)
        .status()
        .unwrap();
    assert!(unlocked.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(4), "{stderr}");
    let said = format!(
        "{} is restored, but moving its previous version to the backup did not finish (",
        agents.display()
    );
    assert!(stderr.contains(&said), "{stderr}");
    let rest = format!(
        "; delete what is left of it at {}\n",
        left.join("AGENTS.md").display()
    );
    assert!(stderr.contains(&rest), "{stderr}");
    assert_eq!(read_text(&agents), "mirror edit\n");
    let copy = the_only_backup(&state.dir).join("projects/work/AGENTS.md");
    assert_eq!(read_text(&copy), "project original\n");
}
