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
