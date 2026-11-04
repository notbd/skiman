//! Fetching upstream commits into the per-upstream bare cache under machine
//! state (spec Source fetching). Only commits are fetched, by full sha, so
//! the commit recorded in the lock is exactly the one that was read, even if
//! a branch moves meanwhile. Nothing is checked out; snapshots are read from
//! the objects (`crate::snapshot`).

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

    /// Makes `commit` (a full sha) available in the cache, fetching it when
    /// missing, and pins it under `refs/skiman/c/<sha>` so it is kept.
    pub fn fetch_commit(&self, commit: &str) -> Result<(), Failure> {
        assert!(is_full_sha(commit), "fetch by full sha only");
        let mut has = self.git();
        has.args(["cat-file", "-e", &format!("{commit}^{{commit}}")]);
        if has.output().is_ok_and(|o| o.status.success()) {
            return Ok(());
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
            &format!("{commit}:refs/skiman/c/{commit}"),
        ]);
        git::run(fetch, &format!("fetching {commit} from {}", self.url))?;
        Ok(())
    }
}

/// 40 lowercase hex digits.
pub fn is_full_sha(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
