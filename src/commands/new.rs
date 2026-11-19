//! `skiman new <name> -d <description> [--group <group>]`: create an own
//! skill, in a group folder when one is given, then sync.

use std::collections::BTreeSet;

use crate::Exit;
use crate::backup::Backups;
use crate::canon::Canon;
use crate::canon_lock::CanonLock;
use crate::change::{Change, Want};
use crate::commands::{ask, has_terminal};
use crate::failure::{Failure, finish};
use crate::layout::skill_path;
use crate::names::{self, Taken};
use crate::paths::Paths;
use crate::rules::{self, Scope};
use crate::snapshot::SnapshotFile;
use crate::sync::{self, Output};
use crate::tree::WorkingTree;

pub const MAX_DESCRIPTION: usize = 1024;

pub fn run(name: &str, description: Option<String>, group: Option<&str>) -> Exit {
    finish(new(name, description, group))
}

fn new(name: &str, description: Option<String>, group: Option<&str>) -> Result<Exit, Failure> {
    if description.is_none() && !has_terminal() {
        return Err(Failure::usage(
            "missing flag without a terminal: -d <description>",
        ));
    }
    let paths = Paths::from_env()?;
    let repo = Canon::from_config(&paths)?;
    names::check_new_name(name)?;
    if let Some(group) = group {
        names::check_group(paths.home(), &repo, group, name)?;
    }
    let description = match description {
        Some(description) => description,
        None => ask("Description: ")?,
    };
    let length = description.chars().count();
    if !(1..=MAX_DESCRIPTION).contains(&length) {
        return Err(Failure::refused(format!(
            "the description must be 1 to {MAX_DESCRIPTION} characters (it has {length})"
        )));
    }

    let lock = CanonLock::wait(&paths, &repo)?;
    // Staged whole, then put in place: never half a skill.
    let skill_md = SnapshotFile {
        path: "SKILL.md".to_owned(),
        executable: false,
        content: skill_md(name, &description).into_bytes(),
    };
    let change = Change {
        pieces: vec![(skill_path(group, name), Want::Folder(vec![skill_md]))],
        command: "new",
        save_files: false,
    };
    // A `new` stopped after the folder was in place finishes when run again.
    let ours = |taken| Ok(taken == Taken::Folder && change.in_place(repo.root())?);
    names::check_not_taken(paths.home(), &repo, name, ours)?;
    // The whole skill folder is new content.
    let scope = Scope {
        skills: BTreeSet::from([name.to_owned()]),
        consistency: BTreeSet::from([name.to_owned()]),
        ..Scope::default()
    };
    rules::validate_write(&change.overlay(&WorkingTree::new(repo.root())), &scope)?;
    change.apply(&repo, &lock, &mut Backups::new(&paths))?;
    Ok(sync::run(&paths, &repo, &lock, Output::Normal))
}

/// A `SKILL.md` that passes every rule: frontmatter with `name` and the
/// description as a double-quoted YAML scalar, so any text stays valid YAML.
fn skill_md(name: &str, description: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {}\n---\n\n# {name}\n",
        yaml_double_quoted(description)
    )
}

fn yaml_double_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
