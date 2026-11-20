//! Linked folders (ADR 0010): folders of skills outside the canon,
//! registered in `skiman.toml` as `[linked.<id>]`, linked into the canon as
//! `linked/<id>` and from there into every tool directory. skiman never
//! writes inside one.
//!
//! [`Linked::read`] reads every registration on this machine once per
//! command; distribution, the project scope, `doctor`, `check`, `list`,
//! `overview`, the commands that take a skill name and the commands that
//! move content into the canon ask it what they need: the skills and the
//! names they hold ([`Linked::names`], [`Linked::skill`]), the write guard
//! ([`Linked::refusal`]), what nothing may be moved out of
//! ([`Linked::reaches_into`]) and which project items overlap a folder.

pub mod canon;
pub mod check;
pub mod folder;
pub mod gitignore;
pub mod path;
pub mod summary;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::canon::Canon;
use crate::failure::Failure;
use crate::fsutil::realpath_as_far_as_exists;
use crate::layout::Layout;
use crate::manifest::Manifest;
use crate::names;
use crate::project::root::expand;
pub use folder::Contents;
use path::{Checked, Listings, Places, Stop, overlap, resolve, resolve_in};
pub use summary::stand_in;

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
    /// `path` with `~` expanded, normalised lexically: what `linked/<id>`
    /// points to. `None` when the table has no usable path.
    pub path: Option<PathBuf>,
    /// `path` resolved link by link, as far as it resolves.
    resolved: Option<PathBuf>,
    /// The links met on the way to `resolved`.
    links: Vec<PathBuf>,
    pub state: State,
}

#[derive(Debug, Clone)]
pub enum State {
    /// The table is not well formed or the path breaks a path rule (exit 3).
    Broken(String),
    /// Nothing is at the path on this machine (exit 0).
    Missing,
    /// The folder, its top-level `SKILL.md` (its shape is then unknown),
    /// an entry's own metadata, or the path to it cannot be read (exit 3):
    /// its links and names stay exactly as they are.
    Unreadable(String),
    Read(Contents),
}

impl Linked {
    /// Reads every `[linked.<id>]` table of `manifest`: checks its path
    /// against the path rules and reads the folder of each that passes.
    pub fn read(home: &Path, repo: &Canon, manifest: &Manifest) -> Linked {
        let places = Places::new(home, repo);
        let mut listings = Listings::default();
        let mut folders: Vec<Folder> = Vec::new();
        for entry in manifest.linked() {
            let mut folder = Folder {
                id: entry.id,
                path: None,
                resolved: None,
                links: Vec::new(),
                state: State::Missing,
            };
            match entry.path {
                Ok(written) => {
                    let path = expand(home, &written).expect("checked by is_path_form");
                    folder.path = Some(path);
                    folder.state = check(&mut folder, &places, &folders, &mut listings);
                }
                Err(malformed) => {
                    // Skipped, but the folder at a usable path stays protected.
                    if let Some(written) = malformed.path {
                        let path = expand(home, &written).expect("checked by is_path_form");
                        folder.resolved = Some(resolve_in(&path, &mut listings).path);
                        folder.path = Some(path);
                    }
                    folder.state = State::Broken(malformed.why);
                }
            }
            folders.push(folder);
        }
        for i in 0..folders.len() {
            if let State::Read(_) = folders[i].state {
                let own = &folders[i];
                let breaks = |p: &Path, checked| child_breaks(p, checked, own, &folders, &places);
                // The realpath: the first link met under it is the entry itself.
                let dir = own.resolved.as_ref().expect("a read folder resolved");
                let state = match folder::read(dir, &breaks, &mut listings) {
                    Ok(contents) => State::Read(contents),
                    Err(e) => State::Unreadable(e),
                };
                folders[i].state = state;
            }
        }
        Linked {
            folders,
            dir: repo.linked_dir(),
            places,
        }
    }

    /// [`Linked::read`] on the canon's own `skiman.toml`.
    pub fn load(home: &Path, repo: &Canon) -> Result<Linked, Failure> {
        Ok(Linked::read(home, repo, &Manifest::load(repo)?))
    }

    /// The linked skill `name`: the first folder, in id order, that holds a
    /// skill of that name (one whose `SKILL.md` cannot be probed included),
    /// and the skill's folder. Distributed or not.
    pub fn skill(&self, name: &str) -> Option<(&Folder, PathBuf)> {
        self.folders.iter().find_map(|folder| {
            let State::Read(contents) = &folder.state else {
                return None;
            };
            // The single-skill shape: the registered path.
            if folder.id == name && contents.single {
                return Some((folder, folder.path.clone().unwrap_or_default()));
            }
            let holds = contents
                .skills
                .iter()
                .chain(contents.unprobed.iter().map(|(n, _)| n))
                .any(|n| n == name);
            holds.then(|| (folder, folder.skill_dir(name)))
        })
    }

    /// The linked skill `name` as messages name it, "a linked skill in <its
    /// folder>, in linked folder `<id>`", and the linked folder's id.
    pub fn described(&self, name: &str) -> Option<(String, &str)> {
        let (folder, dir) = self.skill(name)?;
        Some((
            format!(
                "a linked skill in {}, in linked folder `{}`",
                dir.display(),
                folder.id
            ),
            &folder.id,
        ))
    }

    /// Why a command that acts on a skill in the canon refuses `name`, which
    /// is a linked skill (call it when nothing in the canon has the name),
    /// and the linked folder's id. While the folder cannot be read, the
    /// link `kept` for it still tells whose skill it is.
    pub fn edited_elsewhere<'a>(
        &'a self,
        name: &str,
        kept: &'a BTreeMap<String, Holder>,
    ) -> Option<(String, &'a str)> {
        let edited = "it is edited there, and skiman never changes it";
        if let Some((it, id)) = self.described(name) {
            return Some((format!("{name} is {it}; {edited}"), id));
        }
        let Holder::Kept(id) = kept.get(name)? else {
            return None;
        };
        Some((
            format!(
                "{name} is a linked skill in linked folder `{id}`, which cannot be read now; {edited}"
            ),
            id,
        ))
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
        let folder = self.holding(target)?;
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

    /// The registered folder `target` (a realpath, or resolved as far as it
    /// goes) is, or lies inside; every registration with a path counts.
    pub fn holding(&self, target: &Path) -> Option<&Folder> {
        self.folders.iter().find(|f| f.holds(target))
    }

    /// The registered folder one of whose valid child targets `target` (a
    /// realpath, or resolved as far as it goes) is or lies inside, and that
    /// target.
    pub fn led_to(&self, target: &Path) -> Option<(&Folder, &Path)> {
        self.folders.iter().find_map(|folder| match &folder.state {
            State::Read(contents) => contents
                .targets
                .iter()
                .find(|t| target.starts_with(t))
                .map(|t| (folder, t.as_path())),
            _ => None,
        })
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

    /// Whether sync leaves some links exactly as they are ([`Linked::keeper`]).
    pub fn keeps_any(&self) -> bool {
        self.folders.iter().any(|f| match &f.state {
            State::Unreadable(_) => true,
            State::Read(contents) => !contents.unprobed.is_empty(),
            _ => false,
        })
    }

    /// What keeps a tool-directory link to `target` exactly as it is, with
    /// its name: the folder that cannot be read when `target` lies in its
    /// `<canon>/linked/<id>`, or the linked skill whose `SKILL.md` cannot be
    /// probed when it lies in that skill's target.
    pub fn keeper(&self, target: &Path) -> Option<Holder> {
        self.folders.iter().find_map(|folder| {
            let link = self.dir.join(&folder.id);
            let unprobed = |name: &str| Holder::Unprobed {
                id: folder.id.clone(),
                name: name.to_owned(),
            };
            match &folder.state {
                State::Unreadable(_) => target
                    .starts_with(&link)
                    .then(|| Holder::Kept(folder.id.clone())),
                State::Read(contents) => contents
                    .unprobed
                    .iter()
                    .find(|(name, _)| target.starts_with(self.skill_target(folder, name)))
                    .map(|(name, _)| unprobed(name)),
                _ => None,
            }
        })
    }

    /// The linked skills to distribute, one per name, in id and then name
    /// order, and the ones that are not distributed because another skill
    /// holds their name (spec "Names and collisions"). Only folders `offers`
    /// accepts take part. The names `kept` by the links [`Linked::keeper`]
    /// keeps come first ([`withheld`]); then every skill folder under
    /// `skills/` (`layout`) holds its name, enabled or not, and so does
    /// every group folder (ADR 0009); then the folder whose id sorts
    /// first. A skill whose `SKILL.md` cannot be probed holds a name only
    /// through `kept`, so it never takes a name it did not hold.
    pub fn names(
        &self,
        layout: &Layout,
        kept: &BTreeMap<String, Holder>,
        offers: impl Fn(&str) -> bool,
    ) -> (Vec<(String, PathBuf)>, Vec<Collision>) {
        let mut taken = BTreeMap::new();
        let mut wanted = Vec::new();
        let mut collisions = Vec::new();
        for folder in self.folders.iter().filter(|f| offers(&f.id)) {
            for name in Linked::skill_names(folder) {
                let holder = if let Some(holder) = kept.get(&name) {
                    Some(holder.clone())
                } else if let Some(placed) = layout.find(&name) {
                    Some(Holder::Canon(placed.path()))
                } else if let Some(group) = layout.group_named(&name) {
                    Some(Holder::Group(format!("skills/{group}")))
                } else {
                    taken.get(&name).cloned()
                };
                match holder {
                    Some(holder) => collisions.push(Collision {
                        id: folder.id.clone(),
                        dir: folder.skill_dir(&name),
                        name,
                        holder,
                    }),
                    None => {
                        taken.insert(name.clone(), Holder::Linked(folder.id.clone()));
                        wanted.push((name.clone(), self.skill_target(folder, &name)));
                    }
                }
            }
        }
        (wanted, collisions)
    }

    /// The names of `folder`'s linked skills, in name order, as every
    /// command reads them: the id for a folder that is one skill, none for
    /// a folder that was not read.
    pub fn skill_names(folder: &Folder) -> Vec<String> {
        match &folder.state {
            State::Read(contents) if contents.single => vec![folder.id.clone()],
            State::Read(contents) => contents.skills.clone(),
            _ => Vec::new(),
        }
    }

    /// Whether `folder` was read as one skill.
    pub fn is_single(folder: &Folder) -> bool {
        matches!(&folder.state, State::Read(contents) if contents.single)
    }

    /// What `list` and `overview` show of each folder, in id order (spec
    /// "Display"): one not on this machine, or the linked skills of one that
    /// was read, each with the skill that holds its name when it lost a
    /// collision ([`Linked::names`] with every folder taking part and the
    /// names `kept` in the tool directories). A folder that breaks a rule
    /// or cannot be read is left out: sync, `check` and `doctor` report it.
    pub fn shown(&self, layout: &Layout, kept: &BTreeMap<String, Holder>) -> Vec<(&Folder, Shown)> {
        let (_, collisions) = self.names(layout, kept, |_| true);
        self.folders
            .iter()
            .filter_map(|folder| {
                let shown = match &folder.state {
                    State::Missing => Shown::Missing,
                    State::Read(_) => Shown::Skills(
                        Linked::skill_names(folder)
                            .into_iter()
                            .map(|name| {
                                let lost = collisions
                                    .iter()
                                    .find(|c| c.id == folder.id && c.name == name)
                                    .map(|c| c.holder.clone());
                                (name, lost)
                            })
                            .collect(),
                    ),
                    State::Broken(_) | State::Unreadable(_) => return None,
                };
                Some((folder, shown))
            })
            .collect()
    }
}

/// A linked folder as `list` and `overview` show it.
#[derive(Debug, Clone)]
pub enum Shown {
    /// Nothing is at its path on this machine.
    Missing,
    /// Its linked skills in name order, each with what holds its name when
    /// it is not distributed.
    Skills(Vec<(String, Option<Holder>)>),
}

/// A skill folder under `skills/` that is not distributed: a link `kept`
/// for a linked folder or skill that cannot be read now holds its name
/// (spec "Sync"), so a passing failure never changes who holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Withheld {
    /// Relative to the canon.
    pub path: String,
    pub holder: Holder,
}

impl Withheld {
    pub fn message(&self) -> String {
        format!(
            "skill folder {} is not distributed: {}",
            self.path, self.holder
        )
    }
}

/// Every skill folder under `skills/` (`layout`) of the canon at `root`
/// whose name `kept` holds, that is not `enabled = false` in `manifest`
/// and that is distributable ([`crate::layout::skill_folder`]), in layout
/// order: sync, `check` and `doctor` report each.
pub fn withheld(
    root: &Path,
    layout: &Layout,
    manifest: &Manifest,
    kept: &BTreeMap<String, Holder>,
) -> Vec<Withheld> {
    let entries = manifest.skills();
    layout
        .skills()
        .iter()
        .filter_map(|placed| {
            let holder = kept.get(&placed.name)?;
            let enabled = entries.get(&placed.name).is_none_or(|e| e.enabled);
            let dir = root.join(placed.path());
            let distributable = crate::layout::skill_folder(layout, placed, &dir)
                == crate::layout::SkillFolder::Distributable;
            (enabled && distributable).then(|| Withheld {
                path: placed.path(),
                holder: holder.clone(),
            })
        })
        .collect()
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
    /// A group folder under `skills/`, as a path relative to the canon.
    Group(String),
    /// The linked folder whose id sorts first.
    Linked(String),
    /// A link kept for a linked folder that cannot be read now.
    Kept(String),
    /// A link kept for the linked skill `name` of folder `id`, whose
    /// `SKILL.md` cannot be probed now.
    Unprobed { id: String, name: String },
}

/// What holds the name, as sync, `check`, `doctor`, `list` and `overview` say it.
impl std::fmt::Display for Holder {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Holder::Canon(path) => write!(f, "the skill folder {path} uses the name"),
            Holder::Group(path) => write!(f, "the group folder {path} uses the name"),
            Holder::Linked(id) => write!(f, "linked folder `{id}` sorts first and uses the name"),
            Holder::Kept(id) => write!(
                f,
                "linked folder `{id}`, which cannot be read now, keeps the name"
            ),
            Holder::Unprobed { id, name } => write!(
                f,
                "linked skill {name} of `{id}`, whose SKILL.md cannot be read now, keeps the name"
            ),
        }
    }
}

impl Collision {
    pub fn message(&self) -> String {
        format!(
            "linked skill {} in {} is not distributed: {}",
            self.name,
            self.dir.display(),
            self.holder
        )
    }
}

/// The state of `folder` by the path rules, before its content is read:
/// `Read` with no contents yet when it passes. `earlier` are the
/// registrations whose id sorts first: an overlap breaks the later one.
fn check(
    folder: &mut Folder,
    places: &Places,
    earlier: &[Folder],
    listings: &mut Listings,
) -> State {
    let path = folder.path.clone().expect("set by the caller");
    let resolved = resolve_in(&path, listings);
    folder.resolved = Some(resolved.path.clone());
    folder.links = resolved.links.clone();
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
        .find_map(|link| places.breaks_link(link).map(|why| (link, why)))
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
        let theirs: Vec<&PathBuf> = other.registered().collect();
        // A link met on the way that lies inside the other folder reaches into it.
        let through = |links: &[PathBuf], into: &[&PathBuf]| {
            links.iter().any(|l| into.iter().any(|p| l.starts_with(p)))
        };
        if mine.iter().any(|m| theirs.iter().any(|t| overlap(m, t)))
            || through(&folder.links, &theirs)
            || through(&other.links, &mine)
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

    /// Whether `target` is, or lies inside, the registered path.
    fn holds(&self, target: &Path) -> bool {
        self.registered().any(|p| target.starts_with(p))
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
/// another linked folder nor be or contain its own. A link met on the way
/// that [leads home](Places::leads_home) breaks none.
fn child_breaks(
    path: &Path,
    checked: Checked,
    own: &Folder,
    all: &[Folder],
    places: &Places,
) -> Option<String> {
    if checked == Checked::Link && places.leads_home(path) {
        return None;
    }
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
