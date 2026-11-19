//! The state table (project-scope.md "Sync: the state table"), rows 4 to 15,
//! for one item, and the words every command uses for each state. Rows 1 to 3
//! (the skill repo mid-operation, an unusable root, a mirror path with no item
//! shape) are decided by the callers before an item is classified. Sync,
//! `adopt`, `restore` and `doctor` all call [`classify`], so every command
//! agrees on what an item's state is and how to settle it.

use std::collections::BTreeSet;
use std::path::Path;

use super::command;
use super::content::{self, Content, Side};
use super::item::Item;
use super::pattern::Kind;
use super::root::Root;
use crate::fsutil::broken_chain;
use crate::repo::SkillRepo;
use crate::rules::{self, Scope, Violation};
use crate::tree::{Overlay, WorkingTree};

/// One row of the state table, or one of the two states outside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Row 4: the mirror is gone and the project holds no item: forget the synced hash.
    Forgotten,
    /// Row 5: the mirror entry cannot be used as it is: why.
    MirrorInvalid(String),
    /// Row 6: the item's project folder is gone.
    ProjectGone,
    /// Row 7: the project folder is not on this machine and never was.
    NotHere,
    /// Row 8: something else is at the item's path or on the way to it: what.
    Occupied(String),
    /// Row 9: the mirror holds it, this machine never had it.
    NotOnMachine,
    /// Row 10: deleted in the project since the last sync.
    DeletedInProject,
    /// Row 11: deleted in the mirror since the last sync.
    DeletedInMirror,
    /// Row 12: both sides are equal.
    InSync,
    /// Row 13: the project changed, the mirror did not.
    ProjectChanged,
    /// Row 13, blocked: the project side cannot be taken as it is (why).
    Blocked(String),
    /// Row 14: the mirror changed, the project did not.
    MirrorChanged,
    /// Row 15: both changed, or both differ with nothing recorded.
    Conflict,
    /// On disk only, never adopted: a candidate for `adopt`.
    New,
    /// On neither side, nothing recorded.
    Nothing,
}

impl State {
    /// Whether the state needs the user (exit 3).
    pub fn needs_attention(&self) -> bool {
        matches!(
            self,
            State::MirrorInvalid(_)
                | State::Occupied(_)
                | State::DeletedInProject
                | State::DeletedInMirror
                | State::Blocked(_)
                | State::MirrorChanged
                | State::Conflict
        )
    }

    /// The state in a few words.
    pub fn describe(&self) -> String {
        match self {
            State::MirrorInvalid(why) => format!("its mirror cannot be used: {why}"),
            State::ProjectGone => "its project folder is gone".to_owned(),
            State::NotHere => "its project folder is not on this machine".to_owned(),
            State::Occupied(what) => what.clone(),
            State::NotOnMachine => "it is in the mirror but not in the project yet".to_owned(),
            State::DeletedInProject => "it was deleted in the project".to_owned(),
            State::DeletedInMirror => "it was deleted in the mirror".to_owned(),
            State::Blocked(why) => format!("it is not copied into the mirror: {why}"),
            State::MirrorChanged => "its mirror changed".to_owned(),
            State::Conflict => "it and its mirror both changed".to_owned(),
            State::ProjectChanged => {
                "the project's version is newer; `skiman sync` copies it into the mirror".to_owned()
            }
            State::InSync => "both sides are equal".to_owned(),
            State::New => "it is not adopted yet".to_owned(),
            State::Forgotten | State::Nothing => "there is nothing on either side".to_owned(),
        }
    }
}

/// An item's state, with what was read on the way.
#[derive(Debug, Clone)]
pub struct Evaluated {
    pub state: State,
    /// The project side, when it was read.
    pub disk: Option<Content>,
    pub disk_hash: Option<String>,
    pub mirror_hash: Option<String>,
}

impl Evaluated {
    /// The commands that settle the item, paste-ready; `None` when it
    /// needs nothing.
    pub fn fix(&self, root: &Root, item: &Item) -> Option<String> {
        let disk = [item.disk(root)];
        let cmd = |verb: &str| command(verb, &disk);
        Some(match &self.state {
            State::DeletedInProject => format!(
                "bring it back from the mirror: {}; or stop managing it: {}",
                cmd("restore"),
                cmd("eject")
            ),
            State::DeletedInMirror => format!(
                "take it back in: {}; or stop managing it: {}",
                cmd("adopt"),
                cmd("eject")
            ),
            State::MirrorChanged => format!(
                "write it to the project: {}; or keep the project's version: {}",
                cmd("restore"),
                cmd("adopt")
            ),
            State::Conflict => format!(
                "keep the project's version: {}; or the mirror's: {} (the other goes to a backup)",
                cmd("adopt"),
                cmd("restore")
            ),
            State::Blocked(_) if self.mirror_hash.is_some() => format!(
                "fix it in the project, or bring back the mirror's version: {}",
                cmd("restore")
            ),
            State::Blocked(_) => "fix it in the project".to_owned(),
            State::MirrorInvalid(_) => format!(
                "fix {} in the canon (`skiman check` lists the rules)",
                item.mirror_rel()
            ),
            State::Occupied(_) => format!("move it away, or stop managing it: {}", cmd("eject")),
            State::ProjectGone => format!(
                "if it moved, adopt the new path; to stop managing it: {}",
                command("eject", &[item.anchor_dir(root)])
            ),
            State::NotOnMachine => format!("write it: {}", cmd("restore")),
            _ => return None,
        })
    }

    /// `<path>: <state>; <fix>`: what sync and doctor print for the item.
    pub fn message(&self, root: &Root, item: &Item) -> String {
        let head = format!("{}: {}", item.disk(root).display(), self.state.describe());
        match self.fix(root, item) {
            Some(fix) => format!("{head}; {fix}"),
            None => head,
        }
    }
}

/// [`evaluate`], with a project change that breaks the safety rules (a
/// secret, conflict markers) classified as blocked, as sync treats it.
pub fn classify(
    repo: &SkillRepo,
    root: &Root,
    item: &Item,
    synced: Option<&str>,
) -> std::io::Result<Evaluated> {
    let mut evaluated = evaluate(root, item, repo.root(), synced)?;
    if evaluated.state == State::ProjectChanged
        && let Some(content) = &evaluated.disk
        && let Err(violations) = check_rules(repo, item, content)
    {
        let mut why: Vec<String> = violations.iter().map(ToString::to_string).collect();
        if let Some(hint) = rules::allow_hint(&violations) {
            why.push(hint.split_whitespace().collect::<Vec<_>>().join(" "));
        }
        evaluated.state = State::Blocked(why.join("; "));
    }
    Ok(evaluated)
}

/// The safety rules on `content` as it would sit at the item's mirror.
pub fn check_rules(repo: &SkillRepo, item: &Item, content: &Content) -> Result<(), Vec<Violation>> {
    let tree = WorkingTree::new(repo.root());
    let base = item.mirror_rel();
    let below = |path: &str| {
        if path.is_empty() {
            base.clone()
        } else {
            format!("{base}/{path}")
        }
    };
    let files = content.files();
    let overlay = Overlay::new(
        &tree,
        files
            .iter()
            .map(|(p, _, bytes)| (below(p), bytes.to_vec()))
            .collect(),
    )
    .with_executable(
        files
            .iter()
            .filter(|(_, x, _)| *x)
            .map(|(p, _, _)| below(p))
            .collect(),
    )
    .replacing(vec![base.clone()]);
    let scope = Scope {
        projects: BTreeSet::from([base.clone()]),
        ..Scope::default()
    };
    match rules::check(&overlay, &scope) {
        Ok(violations) if violations.is_empty() => Ok(()),
        Ok(violations) => Err(violations),
        Err(e) => Err(vec![Violation::new(
            base,
            None,
            rules::rule::PROJECT_PATH,
            format!("cannot read the canon: {e}"),
        )]),
    }
}

/// Evaluates `item` of `root`; `repo_root` is the skill repo's realpath and
/// `synced` the item's synced hash (`None` when nothing is recorded).
pub fn evaluate(
    root: &Root,
    item: &Item,
    repo_root: &Path,
    synced: Option<&str>,
) -> std::io::Result<Evaluated> {
    let mirror_path = repo_root.join(item.mirror_rel());
    let mirror = match broken_chain(repo_root, &mirror_path) {
        Some(why) => Side::Unsafe(why),
        None => content::read(&mirror_path, item.kind)?,
    };
    let mirror_hash = match &mirror {
        Side::Present(c) => Some(c.hash()),
        _ => None,
    };
    let disk = read_disk(root, item)?;
    let done = |state: State, disk: Option<Content>| {
        let disk_hash = disk.as_ref().map(Content::hash);
        Ok(Evaluated {
            state,
            disk,
            disk_hash,
            mirror_hash: mirror_hash.clone(),
        })
    };
    let disk_is_item = matches!(
        disk,
        Disk::Side(Side::Present(_)) | Disk::Side(Side::Unsafe(_))
    );
    if mirror == Side::Absent && synced.is_some() && !disk_is_item {
        return done(State::Forgotten, None);
    }
    match &mirror {
        Side::WrongKind => {
            return done(
                State::MirrorInvalid("it is not a regular file or folder".to_owned()),
                None,
            );
        }
        Side::Unsafe(why) => return done(State::MirrorInvalid(why.clone()), None),
        _ => {}
    }
    let disk = match disk {
        Disk::NoAnchor if synced.is_some() => return done(State::ProjectGone, None),
        Disk::NoAnchor => return done(State::NotHere, None),
        Disk::Occupied(what) => return done(State::Occupied(what), None),
        Disk::Side(Side::WrongKind) => {
            let what = match item.kind {
                Kind::File => "regular file",
                Kind::Skill => "real folder",
            };
            return done(
                State::Occupied(format!("{} is not a {what}", item.disk(root).display())),
                None,
            );
        }
        Disk::Side(Side::Unsafe(why)) => return done(State::Blocked(why), None),
        Disk::Side(Side::Absent) => None,
        Disk::Side(Side::Present(content)) => Some(content),
    };
    let state = match (&disk, &mirror_hash, synced) {
        (None, Some(_), None) => State::NotOnMachine,
        (None, Some(_), Some(_)) => State::DeletedInProject,
        (None, None, _) => State::Nothing,
        (Some(_), None, Some(_)) => State::DeletedInMirror,
        (Some(_), None, None) => State::New,
        (Some(d), Some(m), s) => {
            let d = d.hash();
            if &d == m {
                State::InSync
            } else if s == Some(m.as_str()) {
                State::ProjectChanged
            } else if s == Some(d.as_str()) {
                State::MirrorChanged
            } else {
                State::Conflict
            }
        }
    };
    done(state, disk)
}

/// The project side of an item, as far as the state table needs it.
enum Disk {
    /// The item's project folder does not exist.
    NoAnchor,
    /// A path component between the root and the item exists but is not a real folder.
    Occupied(String),
    Side(Side),
}

fn read_disk(root: &Root, item: &Item) -> std::io::Result<Disk> {
    if std::fs::symlink_metadata(item.anchor_dir(root)).is_err() {
        return Ok(Disk::NoAnchor);
    }
    let disk = item.disk(root);
    if let Some(why) = broken_chain(&root.dir, &disk) {
        return Ok(Disk::Occupied(why));
    }
    // A missing intermediate folder means the item is absent.
    Ok(Disk::Side(content::read(&disk, item.kind)?))
}
