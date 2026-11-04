//! Parsing the `<source>` argument of `skiman add` (spec Source fetching):
//! git URLs, GitHub tree and blob URLs, the `<owner>/<repo>` shorthand and
//! local folders. Everything else is refused.

use std::path::{Path, PathBuf};

use crate::failure::Failure;
use crate::fsutil::absolute_lexical;

/// Where `add` installs from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The value written to `source.url`: a git URL, or a local folder as an
    /// absolute or `~` path.
    pub url: String,
    pub kind: SourceKind,
    /// The folder inside the upstream; `""` for the root. For a tree or blob
    /// URL it is known only once the ref is resolved ([`Source::with_path`]).
    pub path: String,
    /// The name of the upstream's root folder (the repo name, or the local
    /// folder's name).
    root_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    /// A git URL fetched into the cache; it follows the default branch unless
    /// `--ref` names another version.
    Git,
    /// A GitHub tree or blob URL: `tail` is the part after `/tree/` or
    /// `/blob/` (without a blob's `SKILL.md`), split on `/`. It starts with
    /// the ref, which may itself contain slashes, and ends with the path.
    Tree { tail: Vec<String> },
    /// A local folder, copied as it is now.
    Local { dir: PathBuf },
}

impl Source {
    /// `home` expands `~`; `cwd` makes a `.`-relative folder absolute.
    pub fn parse(spec: &str, home: &Path, cwd: &Path) -> Result<Source, Failure> {
        if spec.starts_with('-') {
            return Err(Failure::refused(format!(
                "the source `{spec}` starts with `-`; sources never do"
            )));
        }
        if spec.contains("::") {
            return Err(Failure::refused(format!(
                "the source `{spec}` uses a git remote helper (`::`), which skiman never runs"
            )));
        }
        if spec.starts_with('/') || spec.starts_with('~') || spec.starts_with('.') {
            return local(spec, home, cwd);
        }
        if let Some(rest) = spec.strip_prefix("https://github.com/")
            && let [owner, repo, kind @ ("tree" | "blob"), tail @ ..] = segments(rest).as_slice()
        {
            return github_tree(spec, owner, repo, kind, tail);
        }
        if let Some(rest) = spec
            .strip_prefix("https://")
            .or_else(|| spec.strip_prefix("ssh://"))
        {
            return Ok(git(spec, rest.split_once('/').map_or("", |(_, path)| path)));
        }
        if let Some((user_host, path)) = spec.split_once(':')
            && let Some((user, host)) = user_host.split_once('@')
            && !user.is_empty()
            && !host.is_empty()
            && !user_host.contains('/')
            && !path.is_empty()
        {
            return Ok(git(spec, path));
        }
        if let Some((owner, repo)) = spec.split_once('/')
            && let repo = repo.strip_suffix(".git").unwrap_or(repo)
            && is_github_name(owner)
            && is_github_name(repo)
        {
            return Ok(Source {
                url: format!("https://github.com/{owner}/{repo}.git"),
                kind: SourceKind::Git,
                path: String::new(),
                root_name: repo.to_owned(),
            });
        }
        Err(Failure::refused(format!(
            "unsupported source `{spec}`: give an https:// or ssh:// git URL, user@host:path, a local folder starting with `/`, `~` or `.`, a GitHub tree or blob URL, or <owner>/<repo>"
        )))
    }

    /// The same source with its folder inside the upstream set (a tree URL's
    /// path once the ref is split off).
    pub fn with_path(mut self, path: String) -> Source {
        self.path = path;
        self
    }

    /// The name of the folder `path` points at (the root's name for `""`):
    /// a root skill's upstream name when its frontmatter has no `name`.
    pub fn folder_name(&self, path: &str) -> String {
        match path.rsplit('/').next() {
            Some(last) if !last.is_empty() => last.to_owned(),
            _ => self.root_name.clone(),
        }
    }
}

/// The non-empty `/`-separated parts of `path`.
fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// A git URL; `path` is the repository path, whose last part names the root.
fn git(spec: &str, path: &str) -> Source {
    let last = segments(path).last().copied().unwrap_or_default();
    Source {
        url: spec.to_owned(),
        kind: SourceKind::Git,
        path: String::new(),
        root_name: last.strip_suffix(".git").unwrap_or(last).to_owned(),
    }
}

/// `https://github.com/<owner>/<repo>/tree|blob/<ref>/<path>`. A blob URL
/// must point at a `SKILL.md`, and means its folder.
fn github_tree(
    spec: &str,
    owner: &str,
    repo: &str,
    kind: &str,
    tail: &[&str],
) -> Result<Source, Failure> {
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if !is_github_name(owner) || !is_github_name(repo) {
        return Err(Failure::refused(format!(
            "the source `{spec}` does not name a GitHub repository"
        )));
    }
    let mut tail: Vec<String> = tail.iter().map(|s| (*s).to_owned()).collect();
    if kind == "blob" && tail.pop().as_deref() != Some("SKILL.md") {
        return Err(Failure::refused(format!(
            "the blob URL `{spec}` does not point at a SKILL.md; give the URL of the skill's SKILL.md or of its folder"
        )));
    }
    if tail.is_empty() {
        return Err(Failure::refused(format!(
            "the source `{spec}` names no branch, tag or commit after /{kind}/"
        )));
    }
    Ok(Source {
        url: format!("https://github.com/{owner}/{repo}.git"),
        kind: SourceKind::Tree { tail },
        path: String::new(),
        root_name: repo.to_owned(),
    })
}

/// A local folder. `source.url` keeps a `~` path as given (so the record
/// works under another home) and makes a `.`-relative path absolute.
fn local(spec: &str, home: &Path, cwd: &Path) -> Result<Source, Failure> {
    let (url, dir) = if spec == "~" || spec.starts_with("~/") {
        let rest = absolute_lexical(Path::new("/"), Path::new(&spec[1..]));
        let rest = rest.strip_prefix("/").unwrap_or(&rest);
        if rest.as_os_str().is_empty() {
            ("~".to_owned(), home.to_path_buf())
        } else {
            (format!("~/{}", rest.display()), home.join(rest))
        }
    } else if spec.starts_with('~') {
        return Err(Failure::refused(format!(
            "the source `{spec}` uses `~user`; give the folder as an absolute path or below your own `~`"
        )));
    } else {
        let dir = absolute_lexical(cwd, Path::new(spec));
        (dir.display().to_string(), dir)
    };
    let root_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Source {
        url,
        kind: SourceKind::Local { dir },
        path: String::new(),
        root_name,
    })
}

/// A GitHub owner or repository name: ASCII letters, digits, `-`, `_` and
/// `.`, not `.` or `..`.
fn is_github_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}
