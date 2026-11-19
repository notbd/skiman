//! The path rules of linked folders (spec "Path rules"): which paths a
//! registration and a child link may resolve to, and resolving a path link
//! by link so that every link met on the way is checked too.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::fsutil::realpath_as_far_as_exists;
use crate::repo::SkillRepo;
use crate::tools::TOOLS;

/// At most this many links are followed on the way to one path; more is a loop.
const MAX_LINKS: usize = 40;

/// How a path resolved, link by link.
#[derive(Debug)]
pub struct Resolution {
    /// Every link met, at the path it was met at (the links before it resolved).
    pub links: Vec<PathBuf>,
    /// The path with every link that could be read followed. When the
    /// resolution stopped, the rest of the path is joined on unresolved.
    pub path: PathBuf,
    /// Why the resolution stopped before the end; `None` when `path` is the realpath.
    pub stop: Option<Stop>,
}

/// Why a resolution stopped before the end of the path.
#[derive(Debug)]
pub enum Stop {
    /// A component does not exist (or a file stands where a folder is needed).
    Missing,
    /// More than [`MAX_LINKS`] links: a loop.
    Loop,
    /// A component or a link cannot be read (permissions, I/O).
    Unreadable(io::Error),
}

/// Resolves the absolute `path` one component at a time, following every
/// link that can be read, as the system does. Unlike
/// [`realpath_as_far_as_exists`] it goes on through a link whose target is
/// missing, so a chain of links ends at the last target that can be named.
pub fn resolve(path: &Path) -> Resolution {
    let mut done = PathBuf::from("/");
    let mut rest: VecDeque<OsString> = parts(path).collect();
    let mut links = Vec::new();
    let joined = |done: &Path, name: &OsString, rest: &VecDeque<OsString>| {
        rest.iter()
            .fold(done.join(name), |path, part| path.join(part))
    };
    while let Some(name) = rest.pop_front() {
        if name == ".." {
            done.pop();
            continue;
        }
        let next = done.join(&name);
        let stop = match std::fs::symlink_metadata(&next) {
            Ok(meta) if meta.is_symlink() => {
                links.push(next.clone());
                if links.len() > MAX_LINKS {
                    Stop::Loop
                } else {
                    match std::fs::read_link(&next) {
                        Ok(target) => {
                            if target.is_absolute() {
                                done = PathBuf::from("/");
                            }
                            for part in parts(&target).collect::<Vec<_>>().into_iter().rev() {
                                rest.push_front(part);
                            }
                            continue;
                        }
                        Err(e) => Stop::Unreadable(e),
                    }
                }
            }
            Ok(_) => {
                done = next;
                continue;
            }
            Err(e) if is_missing(&e) => Stop::Missing,
            Err(e) => Stop::Unreadable(e),
        };
        return Resolution {
            path: joined(&done, &name, &rest),
            links,
            stop: Some(stop),
        };
    }
    Resolution {
        path: done,
        links,
        stop: None,
    }
}

/// The components of `path` that name something: normal names and `..`.
fn parts(path: &Path) -> impl Iterator<Item = OsString> + '_ {
    path.components().filter_map(|c| match c {
        Component::Normal(name) => Some(name.to_owned()),
        Component::ParentDir => Some("..".into()),
        _ => None,
    })
}

/// `NotFound`, or a file where a folder is needed: nothing is there.
pub fn is_missing(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// HOME and the places skiman writes (the canon, every tool's presence
/// directory and tool directory), each as spelled and as resolved, so a
/// path is compared with both.
#[derive(Debug, Clone)]
pub struct Places {
    homes: Vec<PathBuf>,
    writes: Vec<(PathBuf, String)>,
}

impl Places {
    pub fn new(home: &Path, repo: &SkillRepo) -> Places {
        let both = |path: PathBuf| {
            let real = realpath_as_far_as_exists(&path);
            if real == path {
                vec![path]
            } else {
                vec![path, real]
            }
        };
        let mut writes: Vec<(PathBuf, String)> =
            vec![(repo.root().to_path_buf(), "the canon".to_owned())];
        for tool in TOOLS {
            for path in both(tool.presence_dir(home)) {
                writes.push((path, format!("{}'s presence directory", tool.id)));
            }
            // A tool directory that does not exist yet counts as the realpath
            // of its deepest existing ancestor joined with the rest.
            for path in both(tool.tool_dir(home)) {
                writes.push((path, format!("{}'s tool directory", tool.id)));
            }
        }
        Places {
            homes: both(home.to_path_buf()),
            writes,
        }
    }

    /// Why `path` (absolute) breaks a path rule on its own: it is `/`, HOME
    /// or one of its ancestors, or it lies in or contains a place skiman
    /// writes. `None` when it breaks none.
    pub fn breaks(&self, path: &Path) -> Option<String> {
        if self.homes.iter().any(|home| home.starts_with(path)) {
            return Some(format!(
                "{} is your home folder or one of its parents",
                path.display()
            ));
        }
        self.writes.iter().find_map(|(place, what)| {
            if path.starts_with(place) {
                Some(format!(
                    "{} lies in {}, {what}, where skiman writes",
                    path.display(),
                    place.display()
                ))
            } else if place.starts_with(path) {
                Some(format!(
                    "{} contains {}, {what}, where skiman writes",
                    path.display(),
                    place.display()
                ))
            } else {
                None
            }
        })
    }
}

/// Whether one of `a` and `b` is, or lies inside, the other.
pub fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn root() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        (temp, root)
    }

    #[test]
    fn every_link_on_the_way_is_met_and_followed() {
        let (_temp, root) = root();
        std::fs::create_dir_all(root.join("real/skills")).unwrap();
        symlink(root.join("real"), root.join("one")).unwrap();
        symlink("one/skills", root.join("two")).unwrap();

        let resolved = resolve(&root.join("two"));

        assert_eq!(resolved.links, [root.join("two"), root.join("one")]);
        assert_eq!(resolved.path, root.join("real/skills"));
        assert!(resolved.stop.is_none());
    }

    /// A chain whose last target is missing resolves to that target, where
    /// `realpath_as_far_as_exists` stops at the first link.
    #[test]
    fn a_chain_to_a_missing_target_ends_at_that_target() {
        let (_temp, root) = root();
        std::fs::create_dir(root.join("real")).unwrap();
        symlink(root.join("real/gone"), root.join("one")).unwrap();
        symlink(root.join("one"), root.join("two")).unwrap();

        let resolved = resolve(&root.join("two/sub"));

        assert_eq!(resolved.path, root.join("real/gone/sub"));
        assert!(matches!(resolved.stop, Some(Stop::Missing)));
        assert_eq!(
            realpath_as_far_as_exists(&root.join("two/sub")),
            root.join("two/sub")
        );
    }

    #[test]
    fn a_loop_stops_the_resolution() {
        let (_temp, root) = root();
        symlink(root.join("b"), root.join("a")).unwrap();
        symlink(root.join("a"), root.join("b")).unwrap();
        assert!(matches!(resolve(&root.join("a")).stop, Some(Stop::Loop)));
        symlink("self", root.join("self")).unwrap();
        assert!(matches!(resolve(&root.join("self")).stop, Some(Stop::Loop)));
    }

    #[test]
    fn a_file_in_the_way_counts_as_missing() {
        let (_temp, root) = root();
        std::fs::write(root.join("file"), "").unwrap();
        let resolved = resolve(&root.join("file/sub"));
        assert!(matches!(resolved.stop, Some(Stop::Missing)));
        assert_eq!(resolved.path, root.join("file/sub"));
    }

    #[test]
    fn home_its_parents_and_the_places_skiman_writes_break_the_rules() {
        let (_temp, root) = root();
        let home = root.join("home");
        let repo_dir = root.join("canons/main");
        std::fs::create_dir_all(home.join(".claude/skills")).unwrap();
        std::fs::create_dir_all(&repo_dir).unwrap();
        let repo = SkillRepo::at(&repo_dir).unwrap();
        let places = Places::new(&home, &repo);

        for path in [
            PathBuf::from("/"),
            root.clone(),
            home.clone(),
            home.join(".claude"),
            home.join(".claude/skills/x"),
            // Not there yet: Codex writes to `~/.agents/skills`.
            home.join(".agents"),
            repo_dir.join("sub"),
            // Contains the canon, not HOME.
            root.join("canons"),
        ] {
            assert!(places.breaks(&path).is_some(), "{}", path.display());
        }
        for path in [home.join("work/company"), root.join("elsewhere")] {
            assert_eq!(places.breaks(&path), None, "{}", path.display());
        }
    }
}
