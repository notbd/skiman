//! `skiman new <name> -d <description>`: create an own skill, then sync.

use std::collections::BTreeSet;

use crate::Exit;
use crate::commands::{ask, has_terminal};
use crate::failure::{Failure, finish};
use crate::fsutil;
use crate::names;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope};
use crate::snapshot::SnapshotFile;
use crate::staging::Staging;
use crate::sync::{self, Output};
use crate::tree::{Overlay, WorkingTree};

pub const MAX_DESCRIPTION: usize = 1024;

pub fn run(name: &str, description: Option<String>) -> Exit {
    finish(new(name, description))
}

fn new(name: &str, description: Option<String>) -> Result<Exit, Failure> {
    if description.is_none() && !has_terminal() {
        return Err(Failure::usage(
            "missing flag without a terminal: -d <description>",
        ));
    }
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    names::check_new_name(name)?;
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

    let lock = RepoLock::wait(&repo)?;
    names::check_not_taken(&repo, name)?;
    let content = skill_md(name, &description);
    let rel = format!("skills/{name}/SKILL.md");
    let working_tree = WorkingTree::new(repo.root());
    let candidate = Overlay::new(&working_tree, vec![(rel, content.clone().into_bytes())]);
    // The whole skill folder is new content.
    let scope = Scope {
        skills: BTreeSet::from([name.to_owned()]),
        consistency: BTreeSet::from([name.to_owned()]),
        ..Scope::default()
    };
    rules::validate_write(&candidate, &scope)?;

    // Staged whole, then renamed in: never half a skill, never over one.
    let staging = Staging::begin(&paths, &repo, &lock)?;
    let staged = staging.dir().join("skills").join(name);
    let skill_md = SnapshotFile {
        path: "SKILL.md".to_owned(),
        executable: false,
        content: content.into_bytes(),
    };
    fsutil::write_files(&staged, &[skill_md])?;
    staging.place(&staged, &repo.skill_dir(name))?;
    drop(staging);
    out!(
        "created {}",
        repo.skill_dir(name).join("SKILL.md").display()
    );
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
