//! Fetching upstream commits into the per-upstream bare cache under machine
//! state (spec Source fetching). Only commits are fetched, by full sha, so
//! the commit recorded in the lock is exactly the one that was read, even if
//! a branch moves meanwhile. Nothing is checked out; snapshots are read from
//! the objects (`crate::snapshot`).

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::failure::Failure;
use crate::git::{self, Prompts};
use crate::paths::Paths;
use crate::tree::hex;

/// The bare repository caching one upstream URL:
/// `<state dir>/cache/<sha256 of the URL>.git`.
#[derive(Debug, Clone)]
pub struct Cache {
    url: String,
    dir: PathBuf,
    prompts: Prompts,
}

/// The branch a ref name resolves to and its head commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchHead {
    pub branch: String,
    pub commit: String,
}

impl Cache {
    /// Opens the cache of `url`, creating it on first use. The cache records
    /// `remote.origin.url`, so the user's `includeIf "hasconfig:remote.*.url:..."`
    /// sections apply; fetches still pass the URL itself.
    pub fn open(paths: &Paths, url: &str, prompts: Prompts) -> Result<Cache, Failure> {
        let caches = paths.state_dir().join("cache");
        let dir = caches.join(format!("{}.git", hex(&Sha256::digest(url.as_bytes()))));
        let cache = Cache {
            url: url.to_owned(),
            dir,
            prompts,
        };
        if !cache.dir.join("HEAD").is_file() {
            cache.create(&caches)?;
        }
        Ok(cache)
    }

    /// `git init --bare` under a temporary name, then renamed into place, so
    /// an interrupted creation never leaves a half-made cache.
    fn create(&self, caches: &Path) -> Result<(), Failure> {
        let io = |e: std::io::Error| {
            Failure::operational(format!(
                "cannot create the git cache in {}: {e}",
                caches.display()
            ))
        };
        std::fs::create_dir_all(caches).map_err(io)?;
        let tmp = self
            .dir
            .with_extension(format!("git.tmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut init = self.git_in(&tmp);
        // Files refs, whatever the user's default, so a killed git leaves only
        // the lock files `fetch_commit` removes (an older git ignores this).
        init.args(["-c", "init.defaultRefFormat=files"])
            .env_remove("GIT_DEFAULT_REF_FORMAT");
        init.args(["init", "--quiet", "--bare", "--"]).arg(&tmp);
        git::run(init, "creating the git cache")?;
        let mut config = self.git_in(&tmp);
        config.args(["config", "remote.origin.url", &self.url]);
        git::run(config, "configuring the git cache")?;
        match std::fs::rename(&tmp, &self.dir) {
            Ok(()) => Ok(()),
            // Another skiman process created it first.
            Err(_) if self.dir.join("HEAD").is_file() => {
                let _ = std::fs::remove_dir_all(&tmp);
                Ok(())
            }
            Err(e) => Err(io(e)),
        }
    }

    fn git_in(&self, git_dir: &Path) -> std::process::Command {
        git::fetch_command(git_dir, self.prompts)
    }

    /// A git command on this cache.
    pub fn git(&self) -> std::process::Command {
        self.git_in(&self.dir)
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The upstream's default branch and its head (`ls-remote --symref HEAD`).
    /// An upstream without one (empty, or a detached `HEAD`) is refused.
    pub fn default_branch(&self) -> Result<BranchHead, Failure> {
        let mut cmd = self.git();
        cmd.args(["ls-remote", "--symref", "--", &self.url, "HEAD"]);
        let out = git::run(cmd, &format!("reading the default branch of {}", self.url))?;
        let text = String::from_utf8_lossy(&out);
        let mut branch = None;
        let mut commit = None;
        for line in text.lines() {
            if let Some(target) = line
                .strip_prefix("ref: refs/heads/")
                .and_then(|rest| rest.strip_suffix("\tHEAD"))
            {
                branch = Some(target.to_owned());
            } else if let Some(sha) = line.strip_suffix("\tHEAD")
                && is_full_sha(sha)
            {
                commit = Some(sha.to_owned());
            }
        }
        match (branch, commit) {
            (Some(branch), Some(commit)) => Ok(BranchHead { branch, commit }),
            _ => Err(Failure::refused(format!(
                "{} has no default branch (it is empty or its HEAD is detached)",
                self.url
            ))),
        }
    }

    /// The upstream's branches and tags. Never lists unfiltered: that would
    /// include every `refs/pull/*` of a GitHub repo.
    pub fn refs(&self) -> Result<Refs, Failure> {
        let mut cmd = self.git();
        cmd.args(["ls-remote", "--heads", "--tags", "--", &self.url]);
        let out = git::run(cmd, &format!("listing the refs of {}", self.url))?;
        Ok(Refs::parse(&String::from_utf8_lossy(&out)))
    }

    /// Resolves `--ref <r>`: a full sha is a commit (not looked up); else a
    /// branch, then a tag. A short sha is refused: commits must be given in full.
    pub fn resolve_ref(&self, name: &str) -> Result<ResolvedRef, Failure> {
        if is_full_sha(name) {
            return Ok(commit_ref(name));
        }
        if let Some(resolved) = self.refs()?.resolve(name) {
            return Ok(resolved);
        }
        if name.len() < 40 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Failure::refused(format!(
                "`{name}` is not a branch or tag of {}; a commit must be given as its full 40-hex sha",
                self.url
            )));
        }
        Err(Failure::refused(format!(
            "{} has no branch or tag named `{name}`",
            self.url
        )))
    }

    /// Resolves a tree URL's segments after `/tree/` or `/blob/` into a ref
    /// and the path that follows it.
    pub fn resolve_tree_tail(&self, tail: &[String]) -> Result<(ResolvedRef, String), Failure> {
        if is_full_sha(&tail[0]) {
            return Ok((commit_ref(&tail[0]), tail[1..].join("/")));
        }
        self.refs()?.resolve_prefix(tail).ok_or_else(|| {
            Failure::refused(format!(
                "no branch or tag of {} matches the start of `{}`",
                self.url,
                tail.join("/")
            ))
        })
    }

    /// Makes `commit` (a full sha) available in the cache, fetching it when
    /// missing, and pins it under `refs/skiman/c/<sha>` so it is kept. The
    /// pin, not the commit object, says it was fetched: git writes the pin
    /// once every object is in, while a running or killed fetch may hold the
    /// commit without its tree. Fetches take turns ([`Cache::lock`]); the
    /// lock is held by the skiman fetching and by the git it starts, even
    /// when that skiman alone is killed, so a lock file of git's found by
    /// the one holding it was left by a killed git.
    pub fn fetch_commit(&self, commit: &str) -> Result<(), Failure> {
        assert!(is_full_sha(commit), "fetch by full sha only");
        let pin = format!("refs/skiman/c/{commit}");
        let pinned = || {
            let mut has = self.git();
            has.args(["cat-file", "-e", &pin]);
            has.output().is_ok_and(|o| o.status.success())
        };
        if pinned() {
            return Ok(());
        }
        let lock = self.lock()?;
        if pinned() {
            return Ok(());
        }
        for left in ["shallow.lock".to_owned(), format!("{pin}.lock")] {
            let _ = std::fs::remove_file(self.dir.join(left));
        }
        let mut fetch = self.git();
        fetch.args([
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--depth=1",
            "--",
            &self.url,
            &format!("{commit}:{pin}"),
        ]);
        // git holds the lock as its stdin, which it never reads.
        fetch.stdin(lock);
        git::run(fetch, &format!("fetching {commit} from {}", self.url))?;
        Ok(())
    }

    /// Takes `<state>/skiman/cache/<hex>.git.lock`, the lock of the cache
    /// `<hex>.git`, held until dropped. Waits as long as another skiman
    /// fetches into this cache: a fetch may wait for a password.
    fn lock(&self) -> Result<File, Failure> {
        let path = self.dir.with_extension("git.lock");
        let io = |e: std::io::Error| {
            Failure::operational(format!("cannot lock {}: {e}", path.display()))
        };
        let file = File::create(&path).map_err(io)?;
        if file.try_lock().is_err() {
            let (url, held) = (&self.url, path.display());
            err!(
                "waiting for another skiman, or a git it started, to finish fetching from {url} (it holds {held})"
            );
            file.lock().map_err(io)?;
        }
        Ok(file)
    }
}

/// What a ref names (`ref_kind`). Derived by skiman when it resolves the
/// ref, never taken from a hand-written `ref_kind`; import keeps the one
/// the canon holds for a source it does not fetch again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Branch,
    Tag,
    Commit,
}

impl RefKind {
    /// The `ref_kind` value.
    pub fn as_str(self) -> &'static str {
        match self {
            RefKind::Branch => "branch",
            RefKind::Tag => "tag",
            RefKind::Commit => "commit",
        }
    }
}

/// A ref name resolved against the upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRef {
    /// The ref as `source.ref` records it: a branch or tag name, or a full sha.
    pub name: String,
    pub kind: RefKind,
    pub commit: String,
    /// The name is a branch and also a tag; the branch won.
    pub also_tag: bool,
}

/// The upstream's branches and tags (`ls-remote --heads --tags`), each with
/// its commit; an annotated tag's commit is the peeled `^{}` one.
#[derive(Debug, Clone, Default)]
pub struct Refs {
    branches: BTreeMap<String, String>,
    tags: BTreeMap<String, String>,
}

impl Refs {
    fn parse(text: &str) -> Refs {
        let mut refs = Refs::default();
        let mut peeled = BTreeMap::new();
        for line in text.lines() {
            let Some((sha, name)) = line.split_once('\t') else {
                continue;
            };
            if !is_full_sha(sha) {
                continue;
            }
            if let Some(branch) = name.strip_prefix("refs/heads/") {
                refs.branches.insert(branch.to_owned(), sha.to_owned());
            } else if let Some(tag) = name.strip_prefix("refs/tags/") {
                match tag.strip_suffix("^{}") {
                    Some(tag) => peeled.insert(tag.to_owned(), sha.to_owned()),
                    None => refs.tags.insert(tag.to_owned(), sha.to_owned()),
                };
            }
        }
        refs.tags.extend(peeled);
        refs
    }

    /// The branch or tag `name`; the branch wins when both exist.
    pub fn resolve(&self, name: &str) -> Option<ResolvedRef> {
        let tag = self.tags.get(name);
        let (kind, commit) = match (self.branches.get(name), tag) {
            (Some(commit), _) => (RefKind::Branch, commit),
            (None, Some(commit)) => (RefKind::Tag, commit),
            (None, None) => return None,
        };
        Some(ResolvedRef {
            name: name.to_owned(),
            kind,
            commit: commit.clone(),
            also_tag: kind == RefKind::Branch && tag.is_some(),
        })
    }

    /// Splits a tree URL's `segments` into a ref and a path by longest-prefix
    /// match: a full sha as the first segment is a commit; otherwise the
    /// longest leading run of segments that names a branch or a tag wins.
    pub fn resolve_prefix(&self, segments: &[String]) -> Option<(ResolvedRef, String)> {
        (1..=segments.len()).rev().find_map(|i| {
            let resolved = self.resolve(&segments[..i].join("/"))?;
            Some((resolved, segments[i..].join("/")))
        })
    }
}

fn commit_ref(sha: &str) -> ResolvedRef {
    ResolvedRef {
        name: sha.to_owned(),
        kind: RefKind::Commit,
        commit: sha.to_owned(),
        also_tag: false,
    }
}

/// 40 lowercase hex digits.
pub fn is_full_sha(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
