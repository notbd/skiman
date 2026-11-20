//! The state table (project-scope.md "Sync: the state table"), rows 4 to 15,
//! for one item, and the words every command uses for each state. Rows 1 to 3
//! (the canon mid-operation, an unusable root, a mirror path with no item
//! shape) are decided by the callers before an item is classified. Sync,
//! `adopt`, `restore` and `doctor` all call [`classify`], so every command
//! agrees on what an item's state is. What `adopt` and `restore` do with
//! each state comes only from [`Evaluated::action`]; which of them
//! [`Evaluated::fix`] names is tied to that table by a unit test.

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::Path;

use super::command;
use super::content::{self, Content, Side};
use super::item::Item;
use super::pattern::Kind;
use super::root::Root;
use crate::canon::Canon;
use crate::fsutil::broken_chain;
use crate::rules::{self, Scope, Violation};
use crate::tree::{Overlay, WorkingTree};

/// One row of the state table, or one of the two states outside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
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
    /// On neither side (row 4, which keeps any synced hash: a side that
    /// comes back is then judged against it).
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
            State::Nothing => "there is nothing on either side".to_owned(),
        }
    }
}

/// The two commands that write one side of an item: `adopt` the mirror,
/// `restore` the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Adopt,
    Restore,
}

impl Verb {
    pub fn word(self) -> &'static str {
        match self {
            Verb::Adopt => "adopt",
            Verb::Restore => "restore",
        }
    }

    pub fn done(self) -> &'static str {
        match self {
            Verb::Adopt => "adopted",
            Verb::Restore => "restored",
        }
    }
}

/// What `adopt` or `restore` does with an item in a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Writes it, named exactly or under a folder; what the plan line says.
    Write(&'static str),
    /// Writes it named exactly: it replaces content or undoes a deletion,
    /// which a folder never does, so a folder names the exact path to give.
    Exact(&'static str),
    /// Nothing to write: both sides are equal, or sync copies the project in.
    Settled,
    /// Named exactly, skipped with its state; a folder passes it in
    /// silence: it is the other command's, or nothing is on this machine.
    Elsewhere,
    /// Skipped with its state, also under a folder: it needs fixing first.
    Skip,
}

/// An item's state, with what was read on the way.
#[derive(Debug, Clone)]
pub struct Evaluated {
    pub state: State,
    /// The project side, when it was read.
    pub disk: Option<Hashed>,
    /// The mirror side, when it is a regular item of its kind.
    pub mirror: Option<Hashed>,
}

/// One side of an item and its hash, computed once when it is read.
#[derive(Debug, Clone)]
pub struct Hashed {
    pub content: Content,
    pub hash: String,
}

impl Hashed {
    fn of(content: Content) -> Hashed {
        let hash = content.hash();
        Hashed { content, hash }
    }
}

impl Evaluated {
    /// The state table's column for `verb`.
    pub fn action(&self, verb: Verb) -> Action {
        use Action::*;
        match (verb, &self.state) {
            (_, State::InSync) | (Verb::Adopt, State::ProjectChanged) => Settled,
            (Verb::Adopt, State::New) => Write("adopt"),
            (Verb::Adopt, State::DeletedInMirror | State::MirrorChanged | State::Conflict) => {
                Exact("keep the project's version of")
            }
            (
                Verb::Adopt,
                State::NotOnMachine
                | State::DeletedInProject
                | State::NotHere
                | State::ProjectGone
                | State::Nothing,
            ) => Elsewhere,
            (Verb::Restore, State::NotOnMachine) => Write("write"),
            (Verb::Restore, State::DeletedInProject | State::MirrorChanged | State::Conflict) => {
                Exact("write the mirror's version over")
            }
            (Verb::Restore, State::Blocked(_)) if self.mirror.is_some() => {
                Exact("write the mirror's version over")
            }
            (Verb::Restore, State::New | State::NotHere | State::Nothing) => Elsewhere,
            _ => Skip,
        }
    }

    /// What `verb` writes: the project side for `adopt`, the mirror for `restore`.
    pub fn source(&self, verb: Verb) -> Option<&Hashed> {
        match verb {
            Verb::Adopt => self.disk.as_ref(),
            Verb::Restore => self.mirror.as_ref(),
        }
    }

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
            State::Blocked(_) if self.action(Verb::Restore) != Action::Skip => format!(
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
    repo: &Canon,
    root: &Root,
    item: &Item,
    synced: Option<&str>,
) -> std::io::Result<Evaluated> {
    let mut evaluated = evaluate(root, item, repo.root(), synced)?;
    if evaluated.state == State::ProjectChanged
        && let Some(disk) = &evaluated.disk
        && let Err(violations) = check_rules(repo, item, &disk.content)
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
pub fn check_rules(repo: &Canon, item: &Item, content: &Content) -> Result<(), Vec<Violation>> {
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

/// Evaluates `item` of `root`; `repo_root` is the canon's realpath and
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
    let (mirror, invalid) = match mirror {
        Side::Present(content) => (Some(Hashed::of(content)), None),
        Side::Absent => (None, None),
        Side::WrongKind => (None, Some("it is not a regular file or folder".to_owned())),
        Side::Unsafe(why) => (None, Some(why)),
    };
    let (state, disk) = judge(root, item, mirror.as_ref(), invalid, synced)?;
    Ok(Evaluated {
        state,
        disk,
        mirror,
    })
}

/// The state of `item` given its mirror side (`invalid`: why it cannot be
/// used), and the project side when it was read.
fn judge(
    root: &Root,
    item: &Item,
    mirror: Option<&Hashed>,
    invalid: Option<String>,
    synced: Option<&str>,
) -> std::io::Result<(State, Option<Hashed>)> {
    let disk = read_disk(root, item)?;
    let disk_is_item = matches!(
        disk,
        Disk::Side(Side::Present(_)) | Disk::Side(Side::Unsafe(_))
    );
    // Row 4: checked before row 6, which a gone anchor would match.
    if mirror.is_none() && invalid.is_none() && synced.is_some() && !disk_is_item {
        return Ok((State::Nothing, None));
    }
    if let Some(why) = invalid {
        return Ok((State::MirrorInvalid(why), None));
    }
    let disk = match disk {
        Disk::NoAnchor if synced.is_some() => return Ok((State::ProjectGone, None)),
        Disk::NoAnchor => return Ok((State::NotHere, None)),
        Disk::Occupied(what) => return Ok((State::Occupied(what), None)),
        Disk::Side(Side::WrongKind) => {
            let what = match item.kind {
                Kind::File => "regular file",
                Kind::Skill => "real folder",
            };
            let what = format!("{} is not a {what}", item.disk(root).display());
            return Ok((State::Occupied(what), None));
        }
        Disk::Side(Side::Unsafe(why)) => return Ok((State::Blocked(why), None)),
        Disk::Side(Side::Absent) => None,
        Disk::Side(Side::Present(content)) => Some(Hashed::of(content)),
    };
    let state = match (&disk, mirror, synced) {
        (None, Some(_), None) => State::NotOnMachine,
        (None, Some(_), Some(_)) => State::DeletedInProject,
        (None, None, _) => State::Nothing,
        (Some(_), None, Some(_)) => State::DeletedInMirror,
        (Some(_), None, None) => State::New,
        (Some(d), Some(m), s) => {
            if d.hash == m.hash {
                State::InSync
            } else if s == Some(m.hash.as_str()) {
                State::ProjectChanged
            } else if s == Some(d.hash.as_str()) {
                State::MirrorChanged
            } else {
                State::Conflict
            }
        }
    };
    Ok((state, disk))
}

/// The project side of an item, as far as the state table needs it.
enum Disk {
    /// The item's project folder does not exist.
    NoAnchor,
    /// A path component between the root and the item exists but is not a real folder.
    Occupied(String),
    Side(Side),
}

/// An anchor that cannot be looked at (a folder on the way is `chmod 000`)
/// is an error, not a gone folder: it is still there.
fn read_disk(root: &Root, item: &Item) -> std::io::Result<Disk> {
    match std::fs::symlink_metadata(item.anchor_dir(root)) {
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(Disk::NoAnchor);
        }
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    let disk = item.disk(root);
    if let Some(why) = broken_chain(&root.dir, &disk) {
        return Ok(Disk::Occupied(why));
    }
    // A missing intermediate folder means the item is absent.
    Ok(Disk::Side(content::read(&disk, item.kind)?))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// What sync and doctor tell the user to run is what the table lets that
    /// command do: `fix` names `adopt` or `restore` exactly when the item,
    /// named exactly, is one that command writes. `New` is left out: sync
    /// never reports it.
    #[test]
    fn fix_names_only_the_commands_the_table_lets_write_the_item() {
        let root = Root {
            id: "w".to_owned(),
            written: "/w".to_owned(),
            dir: PathBuf::from("/w"),
            exclude: Vec::new(),
        };
        let item = Item::at(&root, "AGENTS.md", false).expect("an item path");
        let side = || Some(Hashed::of(Content::File(b"x\n".to_vec())));
        let why = || "why".to_owned();
        let states = [
            State::MirrorInvalid(why()),
            State::ProjectGone,
            State::NotHere,
            State::Occupied(why()),
            State::NotOnMachine,
            State::DeletedInProject,
            State::DeletedInMirror,
            State::InSync,
            State::ProjectChanged,
            State::Blocked(why()),
            State::MirrorChanged,
            State::Conflict,
            State::Nothing,
        ];
        for state in states {
            for mirror in [None, side()] {
                let eval = Evaluated {
                    state: state.clone(),
                    disk: side(),
                    mirror,
                };
                let fix = eval.fix(&root, &item).unwrap_or_default();
                for verb in [Verb::Adopt, Verb::Restore] {
                    let writes = matches!(eval.action(verb), Action::Write(_) | Action::Exact(_));
                    let named = fix.contains(&format!("skiman project {} ", verb.word()));
                    assert_eq!(named, writes, "{state:?}, {verb:?}: {fix}");
                }
            }
        }
    }
}
