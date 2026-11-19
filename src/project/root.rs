//! Project roots: the `[projects.<id>]` tables of `skiman.toml`, resolved and
//! validated on this machine (project-scope.md "Configuration"). `skiman.toml`
//! is edited by hand and shared between machines, so every command that uses a
//! root validates it here first.

use std::path::{Path, PathBuf};

use crate::fsutil::{absolute_lexical, realpath_as_far_as_exists};
use crate::manifest::{Manifest, ProjectEntry};
use crate::names;
use crate::repo::SkillRepo;
use crate::tools::TOOLS;

/// A project root that passed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub id: String,
    /// `path` as written in `skiman.toml`.
    pub written: String,
    /// Realpath of the folder.
    pub dir: PathBuf,
    pub exclude: Vec<String>,
}

/// A root that failed validation, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub id: String,
    pub message: String,
    /// The folder simply is not on this machine (the configuration may be
    /// right for another one).
    pub absent: bool,
}

/// `written` (absolute or `~/...`) as an absolute path, lexically normalised;
/// `None` for anything else.
pub fn expand(home: &Path, written: &str) -> Option<PathBuf> {
    let path = if let Some(rest) = written.strip_prefix("~/") {
        home.join(rest)
    } else if written == "~" {
        home.to_path_buf()
    } else if Path::new(written).is_absolute() {
        PathBuf::from(written)
    } else {
        return None;
    };
    Some(absolute_lexical(Path::new("/"), &path))
}

/// `path` written the way `skiman project add` stores it: `~/...` under
/// `HOME` (as given or as its realpath), absolute otherwise.
pub fn written_form(home: &Path, path: &Path) -> String {
    let real_home = home.canonicalize().ok();
    [Some(home), real_home.as_deref()]
        .into_iter()
        .flatten()
        .find_map(|h| {
            path.strip_prefix(h)
                .ok()
                .filter(|r| !r.as_os_str().is_empty())
        })
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|| path.display().to_string())
}

/// Every configured root, validated: `Ok` for the usable ones, `Err` with the
/// reason for the others, in file order.
pub fn resolve(home: &Path, repo: &SkillRepo, manifest: &Manifest) -> Vec<Result<Root, Problem>> {
    let entries = manifest.projects();
    let candidates: Vec<Result<Root, Problem>> = entries
        .iter()
        .map(|entry| check_one(home, repo, entry))
        .collect();
    // A root that is only absent from this machine takes part too, by its
    // path resolved as far as it exists: an overlap is a configuration error
    // wherever it shows. A root that is invalid anyway (`path = "~"`) does not.
    let dirs: Vec<Option<PathBuf>> = candidates
        .iter()
        .zip(&entries)
        .map(|(c, entry)| match c {
            Ok(root) => Some(root.dir.clone()),
            // Resolved as far as it exists, so a link on the way (a linked
            // HOME, a synced folder) compares like the usable roots' realpaths.
            Err(problem) if problem.absent => entry
                .path
                .as_deref()
                .and_then(|p| expand(home, p))
                .map(|p| realpath_as_far_as_exists(&p)),
            Err(_) => None,
        })
        .collect();
    candidates
        .into_iter()
        .enumerate()
        .map(|(i, candidate)| {
            let root = candidate?;
            let overlap = dirs.iter().enumerate().find(|(j, other)| {
                *j != i
                    && other
                        .as_ref()
                        .is_some_and(|o| o.starts_with(&root.dir) || root.dir.starts_with(o))
            });
            match overlap {
                Some((j, _)) => Err(Problem {
                    absent: false,
                    message: format!(
                        "it overlaps project root `{}`; roots must not contain each other",
                        entries[j].id
                    ),
                    id: root.id,
                }),
                None => Ok(root),
            }
        })
        .collect()
}

fn check_one(home: &Path, repo: &SkillRepo, entry: &ProjectEntry) -> Result<Root, Problem> {
    let problem = |message: String| Problem {
        id: entry.id.clone(),
        message,
        absent: false,
    };
    if !names::is_valid(&entry.id) {
        return Err(problem(format!(
            "`{}` is not a valid project root id: use lowercase letters, digits and single hyphens",
            entry.id
        )));
    }
    let Some(written) = entry.path.as_deref() else {
        return Err(problem("it has no `path`".to_owned()));
    };
    let Some(path) = expand(home, written) else {
        return Err(problem(format!(
            "`path = \"{written}\"` must be absolute or start with `~/`"
        )));
    };
    let dir = match path.canonicalize() {
        Ok(dir) if dir.is_dir() => dir,
        Ok(_) => return Err(problem(format!("{} is not a folder", path.display()))),
        Err(_) => {
            return Err(Problem {
                absent: true,
                ..problem(format!("{} is not on this machine", path.display()))
            });
        }
    };
    validate_dir(home, repo, &dir).map_err(problem)?;
    for exclude in &entry.exclude {
        if let Some(why) = invalid_exclude(exclude) {
            return Err(problem(format!("`exclude` entry \"{exclude}\" {why}")));
        }
    }
    Ok(Root {
        id: entry.id.clone(),
        written: written.to_owned(),
        dir,
        exclude: entry.exclude.clone(),
    })
}

/// Why the `exclude` entry `entry` can match nothing; `None` when it can.
pub fn invalid_exclude(entry: &str) -> Option<&'static str> {
    let parts: Vec<&str> = entry.trim_end_matches('/').split('/').collect();
    (entry.starts_with('/')
        || entry.starts_with('~')
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == ".."))
    .then_some("must be a name or a path relative to the root, without empty, `.` or `..` parts")
}

/// The checks on a root folder's realpath that do not depend on other roots.
pub fn validate_dir(home: &Path, repo: &SkillRepo, dir: &Path) -> Result<(), String> {
    let home_real = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    if home_real.starts_with(dir) {
        return Err(format!(
            "{} is your home folder or one of its parents; register project folders instead",
            dir.display()
        ));
    }
    if dir.starts_with(repo.root()) {
        return Err(format!("{} is inside the skill repo", dir.display()));
    }
    for tool in TOOLS {
        let presence = tool.presence_dir(home);
        let presence = presence.canonicalize().unwrap_or(presence);
        if dir.starts_with(&presence) || presence.starts_with(dir) {
            return Err(format!(
                "{} overlaps {}, which holds {}'s global setup",
                dir.display(),
                presence.display(),
                tool.id
            ));
        }
    }
    Ok(())
}

/// The id `skiman project add` proposes for a folder: its name made into a
/// valid name (lowercase, other characters as single hyphens).
pub fn default_id(dir: &Path) -> String {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut id = String::new();
    for c in name.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            id.push(c);
        } else if !id.ends_with('-') && !id.is_empty() {
            id.push('-');
        }
    }
    let id: String = id.chars().take(names::MAX_LEN).collect();
    id.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_accepts_absolute_and_home_paths_only() {
        let home = Path::new("/h");
        assert_eq!(expand(home, "~/w/x"), Some(PathBuf::from("/h/w/x")));
        assert_eq!(expand(home, "/a/../b"), Some(PathBuf::from("/b")));
        assert_eq!(expand(home, "rel"), None);
        assert_eq!(expand(home, "~other/x"), None);
    }

    #[test]
    fn default_ids() {
        assert_eq!(default_id(Path::new("/x/Acme-Corp")), "acme-corp");
        assert_eq!(default_id(Path::new("/x/My Projects_2")), "my-projects-2");
        let long = format!("/x/{} tail", "a".repeat(63));
        assert_eq!(default_id(Path::new(&long)), "a".repeat(63));
    }

    #[test]
    fn written_form_uses_tilde_under_home() {
        let home = Path::new("/h");
        assert_eq!(written_form(home, Path::new("/h/w")), "~/w");
        assert_eq!(written_form(home, Path::new("/o/w")), "/o/w");
    }
}
