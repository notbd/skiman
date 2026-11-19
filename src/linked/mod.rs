//! Linked folders (ADR 0010): folders of skills outside the canon,
//! registered in `skiman.toml` as `[linked.<id>]`, linked into the canon as
//! `linked/<id>` and from there into every tool directory. skiman never
//! writes inside one.
//!
//! [`Linked::read`] reads every registration on this machine once per
//! command; distribution, the project scope, `doctor` and the commands that
//! move content into the canon ask it what they need: the skills, the write
//! guard ([`Linked::refusal`]), what nothing may be moved out of
//! ([`Linked::reaches_into`]) and which project items overlap a folder.

pub mod canon;
pub mod folder;
pub mod gitignore;
pub mod path;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::fsutil::realpath_as_far_as_exists;
use crate::layout::Layout;
use crate::manifest::Manifest;
use crate::names;
use crate::project::root::expand;
use crate::repo::SkillRepo;
pub use folder::Contents;
use path::{Places, Stop, overlap, resolve};

/// The canon's folder of links to linked folders.
pub const LINKED_DIR: &str = "linked";

/// Every registered linked folder as it is on this machine, in id order.
#[derive(Debug, Clone)]
pub struct Linked {
    pub folders: Vec<Folder>,
    /// `<canon>/linked`.
    dir: PathBuf,
    places: Places,
}

/// One `[linked.<id>]` registration on this machine.
#[derive(Debug, Clone)]
pub struct Folder {
    pub id: String,
    /// `path` as written in `skiman.toml`, when the table has a usable one.
    pub written: Option<String>,
    /// `path` with `~` expanded, normalised lexically: what `linked/<id>`
    /// points to. `None` when the table has no usable path.
    pub path: Option<PathBuf>,
    /// `path` resolved link by link, as far as it resolves.
    resolved: Option<PathBuf>,
    pub state: State,
}

#[derive(Debug, Clone)]
pub enum State {
    /// The table is not well formed or the path breaks a path rule (exit 3).
    Broken(String),
    /// Nothing is at the path on this machine (exit 0).
    Missing,
    /// The folder, an entry's own metadata, or the path to it cannot be
    /// read (exit 3): its links and names stay exactly as they are.
    Unreadable(String),
    Read(Contents),
}

impl Linked {
    /// Reads every `[linked.<id>]` table of `manifest`: checks its path
    /// against the path rules and reads the folder of each that passes.
    pub fn read(home: &Path, repo: &SkillRepo, manifest: &Manifest) -> Linked {
        let places = Places::new(home, repo);
        let mut folders: Vec<Folder> = Vec::new();
        for entry in manifest.linked() {
            let mut folder = Folder {
                id: entry.id,
                written: None,
                path: None,
                resolved: None,
                state: State::Missing,
            };
            match entry.path {
                Ok(written) => {
                    let path = expand(home, &written).expect("checked by is_path_form");
                    folder.written = Some(written);
                    folder.path = Some(path);
                    folder.state = check(&mut folder, &places, &folders);
                }
                Err(why) => folder.state = State::Broken(why),
            }
            folders.push(folder);
        }
        for i in 0..folders.len() {
            if let State::Read(_) = folders[i].state {
                let own = &folders[i];
                let breaks = |p: &Path| child_breaks(p, own, &folders, &places);
                let dir = own.path.as_ref().expect("a read folder has a path");
                let state = match folder::read(dir, &breaks) {
                    Ok(contents) => State::Read(contents),
                    Err(e) => State::Unreadable(e),
                };
                folders[i].state = state;
            }
        }
        Linked {
            folders,
            dir: repo.root().join(LINKED_DIR),
            places,
        }
    }

    /// `<canon>/linked/<id>/<name>`, or `<canon>/linked/<id>` for a folder
    /// that is itself one skill: the target of a linked skill's tool-directory link.
    pub fn skill_target(&self, folder: &Folder, name: &str) -> PathBuf {
        let link = self.dir.join(&folder.id);
        match &folder.state {
            State::Read(contents) if contents.single => link,
            _ => link.join(name),
        }
    }

    /// `<canon>/linked`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The registered folder whose path, or one of whose valid child
    /// targets, `resolved` (a realpath, or resolved as far as it goes) is,
    /// contains or lies inside (spec "Nothing is moved out of a linked
    /// folder"). Every registration with a path counts, a broken one too.
    pub fn reaches_into(&self, resolved: &Path) -> Option<&Folder> {
        self.folders.iter().find(|folder| {
            let targets = match &folder.state {
                State::Read(contents) => contents.targets.as_slice(),
                _ => &[],
            };
            folder
                .registered()
                .chain(targets)
                .any(|p| overlap(p, resolved))
        })
    }

    /// Why skiman must not write at `path`, which resolves to `target`
    /// ([`written_through`] or [`location`]): `target` is, or lies inside,
    /// a registered path (spec "Write targets never reach into a linked
    /// folder"). Every registration with a path counts, one that breaks a
    /// rule too. `None` when the write may go ahead.
    pub fn refusal(&self, path: &Path, target: &Path) -> Option<String> {
        let folder = self
            .folders
            .iter()
            .find(|f| f.registered().any(|p| target.starts_with(p)))?;
        let shown = folder
            .path
            .as_ref()
            .expect("a registered folder has a path");
        Some(format!(
            "{} resolves to {}, inside linked folder `{}` ({}), so skiman writes nothing there; point it elsewhere, then run the command again",
            path.display(),
            target.display(),
            folder.id,
            shown.display()
        ))
    }

    /// The first folder that cannot be read: while one cannot, its child
    /// links are unknown, so nothing may be moved out of it and project
    /// items are not changed.
    pub fn unreadable(&self) -> Option<&Folder> {
        self.folders
            .iter()
            .find(|f| matches!(f.state, State::Unreadable(_)))
    }

    /// Why the project item at `path` is neither mirrored nor restored
    /// (spec "Project items"): [`Linked::reaches_into`] on its path
    /// resolved link by link, so a missing item behind a chain of links is
    /// compared where the links lead.
    pub fn item_overlap(&self, path: &Path) -> Option<String> {
        let folder = self.reaches_into(&resolve(path).path)?;
        Some(format!(
            "it overlaps linked folder `{}` ({}), whose content skiman never copies or writes",
            folder.id,
            folder
                .path
                .as_ref()
                .expect("a registered folder has a path")
                .display()
        ))
    }

    /// Whether `path` (a realpath) could be registered as a linked folder
    /// now: it breaks no path rule and overlaps no registered folder.
    pub fn admits(&self, path: &Path) -> bool {
        self.places.breaks(path).is_none() && self.reaches_into(path).is_none()
    }

    /// Link targets sync leaves exactly as they are, and everything below
    /// them: `<canon>/linked/<id>` of each folder that cannot be read, and
    /// the target of each linked skill whose `SKILL.md` cannot be probed.
    pub fn frozen(&self) -> Vec<PathBuf> {
        let mut frozen = Vec::new();
        for folder in &self.folders {
            match &folder.state {
                State::Unreadable(_) => frozen.push(self.dir.join(&folder.id)),
                State::Read(contents) => frozen.extend(
                    contents
                        .unprobed
                        .iter()
                        .map(|(name, _)| self.skill_target(folder, name)),
                ),
                _ => {}
            }
        }
        frozen
    }

    /// The id of the unreadable folder a link target lies in, when it does:
    /// the link keeps its name for that folder.
    pub fn keeper(&self, target: &Path) -> Option<&str> {
        self.folders
            .iter()
            .filter(|f| matches!(f.state, State::Unreadable(_)))
            .find(|f| target.starts_with(self.dir.join(&f.id)))
            .map(|f| f.id.as_str())
    }

    /// The linked skills to distribute, one per name, in id and then name
    /// order, and the ones that are not distributed because another skill
    /// holds their name (spec "Names and collisions"). Only folders `offers`
    /// accepts take part. Every skill folder under `skills/` (`layout`)
    /// holds its name, enabled or not; then the names `kept` (name to id) by
    /// links of unreadable folders and by skills whose `SKILL.md` cannot be
    /// probed; then the folder whose id sorts first.
    pub fn names(
        &self,
        layout: &Layout,
        kept: &BTreeMap<String, String>,
        offers: impl Fn(&str) -> bool,
    ) -> (Vec<(String, PathBuf)>, Vec<Collision>) {
        let mut kept = kept.clone();
        for folder in &self.folders {
            if let State::Read(contents) = &folder.state {
                for (name, _) in &contents.unprobed {
                    kept.entry(name.clone())
                        .or_insert_with(|| folder.id.clone());
                }
            }
        }
        let mut taken: BTreeMap<String, String> = BTreeMap::new();
        let mut wanted = Vec::new();
        let mut collisions = Vec::new();
        for folder in self.folders.iter().filter(|f| offers(&f.id)) {
            let State::Read(contents) = &folder.state else {
                continue;
            };
            let names: Vec<String> = if contents.single {
                vec![folder.id.clone()]
            } else {
                contents.skills.clone()
            };
            for name in names {
                let holder = if let Some(placed) = layout.find(&name) {
                    Some(Holder::Canon(placed.path()))
                } else if let Some(id) = kept.get(&name) {
                    Some(Holder::Kept(id.clone()))
                } else {
                    taken.get(&name).map(|id| Holder::Linked(id.clone()))
                };
                match holder {
                    Some(holder) => collisions.push(Collision {
                        id: folder.id.clone(),
                        dir: folder.skill_dir(&name),
                        name,
                        holder,
                    }),
                    None => {
                        taken.insert(name.clone(), folder.id.clone());
                        wanted.push((name.clone(), self.skill_target(folder, &name)));
                    }
                }
            }
        }
        (wanted, collisions)
    }
}

/// A linked skill that is not distributed: another skill holds its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub id: String,
    pub name: String,
    /// The linked skill's folder (the registered path for a single skill).
    pub dir: PathBuf,
    pub holder: Holder,
}

/// What holds a name a linked skill wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holder {
    /// A skill folder under `skills/`, as a path relative to the canon.
    Canon(String),
    /// The linked folder whose id sorts first.
    Linked(String),
    /// A link kept for a linked folder that cannot be read now.
    Kept(String),
}

impl Collision {
    pub fn message(&self) -> String {
        let holder = match &self.holder {
            Holder::Canon(path) => format!("the skill folder {path} uses the name"),
            Holder::Linked(id) => format!("linked folder `{id}` sorts first and uses the name"),
            Holder::Kept(id) => {
                format!("linked folder `{id}`, which cannot be read now, keeps the name")
            }
        };
        format!(
            "linked skill {} in {} is not distributed: {holder}",
            self.name,
            self.dir.display()
        )
    }
}

/// The state of `folder` by the path rules, before its content is read:
/// `Read` with no contents yet when it passes. `earlier` are the
/// registrations whose id sorts first: an overlap breaks the later one.
fn check(folder: &mut Folder, places: &Places, earlier: &[Folder]) -> State {
    let path = folder.path.clone().expect("set by the caller");
    let resolved = resolve(&path);
    folder.resolved = Some(resolved.path.clone());
    if let Some(why) = places.breaks(&resolved.path) {
        return State::Broken(why);
    }
    // The realpath passes: the path as written or a link on the way does not.
    let hint = match resolved.stop {
        None => format!(
            "; register its realpath {} instead",
            resolved.path.display()
        ),
        Some(_) => String::new(),
    };
    if let Some(why) = places.breaks(&path) {
        return State::Broken(format!("{why}{hint}"));
    }
    if let Some((link, why)) = resolved
        .links
        .iter()
        .find_map(|link| places.breaks(link).map(|why| (link, why)))
    {
        return State::Broken(format!(
            "{} goes through the link {}: {why}{hint}",
            path.display(),
            link.display()
        ));
    }
    let state = match resolved.stop {
        Some(Stop::Missing) => State::Missing,
        Some(Stop::Loop) => {
            return State::Broken(format!("{} goes through a link that loops", path.display()));
        }
        Some(Stop::Unreadable(e)) => {
            State::Unreadable(format!("cannot check {}: {e}", resolved.path.display()))
        }
        None => match std::fs::metadata(&resolved.path) {
            Ok(meta) if meta.is_dir() => State::Read(Contents::default()),
            Ok(_) => return State::Broken(format!("{} is not a folder", path.display())),
            Err(e) => State::Unreadable(format!("cannot check {}: {e}", path.display())),
        },
    };
    let mine = [&path, &resolved.path];
    for other in earlier.iter().filter(|f| f.takes_part()) {
        let theirs = [other.path.as_ref(), other.resolved.as_ref()];
        if mine
            .iter()
            .any(|m| theirs.iter().flatten().any(|t| overlap(m, t)))
        {
            return State::Broken(format!(
                "{} overlaps linked folder `{}`; linked folders must not contain each other",
                path.display(),
                other.id
            ));
        }
    }
    state
}

impl Folder {
    /// The registered path as written (`~` expanded) and as resolved.
    fn registered(&self) -> impl Iterator<Item = &PathBuf> {
        [self.path.as_ref(), self.resolved.as_ref()]
            .into_iter()
            .flatten()
    }

    /// The folder of the linked skill `name`: the registered path itself
    /// for a folder that is one skill.
    pub fn skill_dir(&self, name: &str) -> PathBuf {
        let path = self.path.clone().unwrap_or_default();
        match &self.state {
            State::Read(contents) if contents.single => path,
            _ => path.join(name),
        }
    }

    /// Whether the folder counts against later registrations that overlap it:
    /// any that does not break a rule itself, here or not.
    fn takes_part(&self) -> bool {
        !matches!(self.state, State::Broken(_))
    }
}

/// Why a child link of `own` that resolves through `path` breaks the path
/// rules: the rules on any registered path, and it must neither overlap
/// another linked folder nor be or contain its own.
fn child_breaks(path: &Path, own: &Folder, all: &[Folder], places: &Places) -> Option<String> {
    if let Some(why) = places.breaks(path) {
        return Some(why);
    }
    let own_paths = [own.path.as_ref(), own.resolved.as_ref()];
    if own_paths.iter().flatten().any(|p| p.starts_with(path)) {
        return Some(format!(
            "{} is or contains its own linked folder",
            path.display()
        ));
    }
    all.iter()
        .filter(|f| f.id != own.id && f.takes_part())
        .find(|f| {
            [f.path.as_ref(), f.resolved.as_ref()]
                .iter()
                .flatten()
                .any(|p| overlap(p, path))
        })
        .map(|f| format!("{} overlaps linked folder `{}`", path.display(), f.id))
}

/// The stand-in for a missing summary (ADR 0011): the first sentence of
/// `description`, up to the first `.`, `!` or `?` followed by whitespace or
/// the end, the first `。`, `！` or `？`, or the first line break, whichever
/// comes first; cut to 120 characters with `…`.
pub fn stand_in(description: &str) -> String {
    let chars: Vec<char> = description.trim().chars().collect();
    let mut end = chars.len();
    for (i, c) in chars.iter().enumerate() {
        let next_blank = chars.get(i + 1).is_none_or(|n| n.is_whitespace());
        if matches!(c, '。' | '！' | '？') || (matches!(c, '.' | '!' | '?') && next_blank) {
            end = i + 1;
            break;
        }
        if matches!(c, '\n' | '\r') {
            end = i;
            break;
        }
    }
    let sentence: String = chars[..end].iter().collect();
    let sentence = sentence.trim();
    if sentence.chars().count() > STAND_IN_MAX {
        let cut: String = sentence.chars().take(STAND_IN_MAX - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        sentence.to_owned()
    }
}

/// The longest stand-in, in characters.
const STAND_IN_MAX: usize = 120;

/// Where a write through `path` lands (a tool directory, a settings file,
/// a linked `.gitignore`): its realpath as far as it exists.
pub fn written_through(path: &Path) -> PathBuf {
    realpath_as_far_as_exists(path)
}

/// Where a link or file placed at `path` lands (an instruction location, a
/// link skiman places or removes): the realpath of its parent as far as it
/// exists, joined with its name.
pub fn location(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("/"));
    realpath_as_far_as_exists(parent).join(path.file_name().unwrap_or_default())
}

/// Why `id` cannot name a linked folder; `None` when it can. An id follows
/// the skill-name rule and is not reserved: a single-skill folder's skill
/// is named after it.
pub fn invalid_id(id: &str) -> Option<String> {
    if names::is_reserved(id) {
        Some(format!(
            "`{id}` is a reserved name and cannot be a linked folder id"
        ))
    } else if !names::is_valid(id) {
        Some(format!(
            "`{id}` is not a valid linked folder id: use lowercase letters, digits and single hyphens"
        ))
    } else {
        None
    }
}

/// Whether `path` has the form of a registered path: absolute or starting with `~/`.
pub fn is_path_form(path: &str) -> bool {
    path.starts_with('/') || path.starts_with("~/")
}
