//! Linked folders (ADR 0010, spec #37): folders of skills outside the canon,
//! registered as `[linked.<id>]`, linked into the canon as `linked/<id>` and
//! from there into every tool directory. One module per spec section.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::support::{Sandbox, skill_md, tree, write};

mod canon;
mod case;
mod check;
mod commands;
mod display;
mod distribution;
mod doctor;
mod gitignore;
mod import;
mod missing;
mod names;
mod paths;
mod projects;
mod read_only;
mod switch;

/// A folder of skills at `rel` below `HOME`, holding a skill for each of `names`.
pub fn folder(sandbox: &Sandbox, rel: &str, names: &[&str]) -> PathBuf {
    let dir = sandbox.home().join(rel);
    std::fs::create_dir_all(&dir).unwrap();
    for name in names {
        write(dir.join(name).join("SKILL.md"), skill_md(name));
    }
    dir
}

/// The table that registers [`folder`] `work/company` as `company`, as
/// [`append_manifest`] adds it.
pub const COMPANY: &str = "\n[linked.company]\npath = \"~/work/company\"\n";

/// Runs `skiman sync` and returns its exit code and stderr.
pub fn sync(sandbox: &Sandbox) -> (i32, String) {
    let output = sandbox.skiman().arg("sync").output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// Appends `text` to the canon's `skiman.toml`, as a user editing it by hand.
pub fn append_manifest(sandbox: &Sandbox, text: &str) {
    let path = sandbox.canon().join("skiman.toml");
    let mut old = std::fs::read_to_string(&path).unwrap();
    old.push_str(text);
    std::fs::write(path, old).unwrap();
}

/// Everything below each of `folders`: compare two calls to prove that
/// skiman wrote nothing inside them.
pub fn contents(folders: &[&Path]) -> BTreeMap<PathBuf, BTreeMap<PathBuf, String>> {
    folders
        .iter()
        .map(|dir| (dir.to_path_buf(), tree(dir)))
        .collect()
}

/// A canon with the single skill `~/work/solo` registered and synced, then
/// its top-level `SKILL.md` made a link into `~/work/locked` and a skill
/// `solo/kid` added: once `locked` has mode 000, the top-level `SKILL.md`
/// cannot be probed and the tool directories keep the `solo` link. Returns
/// `locked`.
pub fn unprobed_single(sandbox: &Sandbox) -> PathBuf {
    crate::support::init(sandbox);
    let work = sandbox.home().join("work");
    folder(sandbox, "work", &["solo"]);
    append_manifest(sandbox, "\n[linked.solo]\npath = \"~/work/solo\"\n");
    sandbox.skiman().arg("sync").assert().code(0);
    let locked = work.join("locked");
    write(locked.join("SKILL.md"), skill_md("solo"));
    let top = work.join("solo/SKILL.md");
    std::fs::remove_file(&top).unwrap();
    std::os::unix::fs::symlink(locked.join("SKILL.md"), top).unwrap();
    write(work.join("solo/kid/SKILL.md"), skill_md("kid"));
    locked
}
