//! Parsing the `<source>` argument of `skiman add` (spec Source fetching).
//! Only the GitHub shorthand `<owner>/<repo>` is accepted so far; tree URLs,
//! other git URLs and local folders are #12.

use crate::failure::Failure;

/// Where `add` installs from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The git URL written to `source.url`.
    pub url: String,
    /// The folder inside the upstream; `""` for the root.
    pub path: String,
    /// The name of the folder the source points at (the repo name for the
    /// root): a skill's upstream name when its frontmatter has no `name`.
    pub folder_name: String,
}

impl Source {
    pub fn parse(spec: &str) -> Result<Source, Failure> {
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
        if let Some((owner, repo)) = spec.split_once('/')
            && let repo = repo.strip_suffix(".git").unwrap_or(repo)
            && is_github_name(owner)
            && is_github_name(repo)
        {
            return Ok(Source {
                url: format!("https://github.com/{owner}/{repo}.git"),
                path: String::new(),
                folder_name: repo.to_owned(),
            });
        }
        Err(Failure::refused(format!(
            "unsupported source `{spec}`: give a GitHub repository as <owner>/<repo>"
        )))
    }
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
