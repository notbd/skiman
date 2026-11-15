//! `skiman diff <name>`: the local modifications of a third-party skill, as a
//! unified diff of `upstream/<name>` (the upstream snapshot) against
//! `skills/<name>`. Like the tree hash, only regular files take part and
//! ignored names ([`crate::tree::IGNORED_NAMES`]) are left out. Read-only;
//! takes no lock and spawns no `git`.

use std::collections::BTreeMap;
use std::io::Write;

use similar::TextDiff;

use crate::Exit;
use crate::failure::{Failure, finish};
use crate::manifest::Manifest;
use crate::names;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::tree::{Kind, Tree, WorkingTree, is_ignored};

pub fn run(name: &str) -> Exit {
    finish(diff(name))
}

fn diff(name: &str) -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    let tree = WorkingTree::new(repo.root());
    let io = |e| Failure::operational(format!("cannot read {}: {e}", repo.root().display()));
    let skill = repo.layout()?.path(name);
    let upstream = format!("upstream/{name}");
    if !names::is_valid(name) || tree.kind(&skill).map_err(io)? != Some(Kind::Dir) {
        return Err(Failure::refused(format!(
            "there is no skill folder {skill}"
        )));
    }
    if Manifest::load(&repo)?.source(name).is_none() {
        return Err(Failure::refused(format!(
            "{name} is an own skill: it has no upstream snapshot to compare with"
        )));
    }
    if tree.kind(&upstream).map_err(io)? != Some(Kind::Dir) {
        return Err(Failure::refused(format!(
            "{upstream} does not exist, so there is nothing to compare {skill} with; run `skiman check`"
        )));
    }
    let old = files(&tree, &upstream).map_err(io)?;
    let new = files(&tree, &skill).map_err(io)?;
    let mut rels: Vec<&String> = old.keys().chain(new.keys()).collect();
    rels.sort();
    rels.dedup();
    for rel in rels {
        let (old_path, new_path) = (format!("{upstream}/{rel}"), format!("{skill}/{rel}"));
        let old_file = old.get(rel);
        let new_file = new.get(rel);
        if let (Some(o), Some(n)) = (old_file, new_file)
            && o.executable != n.executable
        {
            out!(
                "mode of {new_path} changed from {} to {}",
                o.mode(),
                n.mode()
            );
        }
        let old_content = match old_file {
            Some(_) => tree.read(&old_path).map_err(io)?,
            None => Vec::new(),
        };
        let new_content = match new_file {
            Some(_) => tree.read(&new_path).map_err(io)?,
            None => Vec::new(),
        };
        if old_file.is_some() && new_file.is_some() && old_content == new_content {
            continue;
        }
        let old_label = if old_file.is_some() {
            old_path
        } else {
            "/dev/null".to_owned()
        };
        let new_label = if new_file.is_some() {
            new_path
        } else {
            "/dev/null".to_owned()
        };
        match (text(&old_content), text(&new_content)) {
            (Some(o), Some(n)) => {
                let _ = write!(
                    std::io::stdout(),
                    "{}",
                    TextDiff::from_lines(o, n)
                        .unified_diff()
                        .header(&old_label, &new_label)
                );
            }
            _ => out!("Binary files {old_label} and {new_label} differ"),
        }
    }
    Ok(Exit::Success)
}

struct File {
    executable: bool,
}

impl File {
    fn mode(&self) -> &'static str {
        if self.executable { "100755" } else { "100644" }
    }
}

/// The regular files below `dir`, by path relative to it.
fn files(tree: &dyn Tree, dir: &str) -> std::io::Result<BTreeMap<String, File>> {
    let skip = dir.len() + 1;
    Ok(tree
        .walk(dir)?
        .into_iter()
        .filter(|entry| !is_ignored(&entry.path))
        .filter_map(|entry| match entry.kind {
            Kind::File { executable, .. } => {
                Some((entry.path[skip..].to_owned(), File { executable }))
            }
            _ => None,
        })
        .collect())
}

/// The content as text, or `None` for a binary file (NUL byte or not UTF-8).
fn text(content: &[u8]) -> Option<&str> {
    if content.contains(&0) {
        return None;
    }
    std::str::from_utf8(content).ok()
}
