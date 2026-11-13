use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A local git repo acting as a GitHub upstream. Its working tree is edited
/// with the methods below; `commit` records the changes.
pub struct Upstream {
    envs: Vec<(OsString, OsString)>,
    dir: PathBuf,
    slug: String,
}

impl Upstream {
    pub(crate) fn init(envs: Vec<(OsString, OsString)>, upstreams: PathBuf, slug: &str) -> Self {
        assert!(
            slug.split('/').count() == 2,
            "upstream slug must be owner/repo, got {slug}"
        );
        let dir = upstreams.join(format!("{slug}.git"));
        std::fs::create_dir_all(&dir).expect("create upstream dir");
        let upstream = Upstream {
            envs,
            dir,
            slug: slug.to_owned(),
        };
        upstream.git(["init", "--quiet", "--initial-branch=main"]);
        upstream
    }

    /// The repo's working tree on disk.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// `https://github.com/<owner>/<repo>.git`.
    pub fn url(&self) -> String {
        format!("https://github.com/{}.git", self.slug)
    }

    /// `https://github.com/<owner>/<repo>/tree/<git_ref>/<path>`.
    pub fn tree_url(&self, git_ref: &str, path: &str) -> String {
        format!("https://github.com/{}/tree/{git_ref}/{path}", self.slug)
    }

    /// Writes a file in the working tree, creating parent folders.
    pub fn write(&self, rel: &str, contents: impl AsRef<[u8]>) -> &Self {
        let path = self.dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create parent dir");
        std::fs::write(&path, contents).expect("write upstream file");
        self
    }

    /// Writes a file with the executable bit set.
    pub fn write_executable(&self, rel: &str, contents: impl AsRef<[u8]>) -> &Self {
        self.write(rel, contents);
        std::fs::set_permissions(self.dir.join(rel), std::fs::Permissions::from_mode(0o755))
            .expect("chmod upstream file");
        self
    }

    /// Creates a symlink at `rel` pointing at `target` (taken literally).
    pub fn symlink(&self, rel: &str, target: &str) -> &Self {
        let path = self.dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create parent dir");
        std::os::unix::fs::symlink(target, path).expect("create upstream symlink");
        self
    }

    /// Removes a file or folder from the working tree.
    pub fn remove(&self, rel: &str) -> &Self {
        let path = self.dir.join(rel);
        if path.is_dir() && !path.is_symlink() {
            std::fs::remove_dir_all(path).expect("remove upstream dir");
        } else {
            std::fs::remove_file(path).expect("remove upstream file");
        }
        self
    }

    /// Commits every change in the working tree and returns the commit sha.
    pub fn commit(&self, message: &str) -> String {
        self.git(["add", "--all"]);
        self.git(["commit", "--quiet", "--allow-empty", "-m", message]);
        self.head()
    }

    /// Creates branch `name` at HEAD without switching to it.
    pub fn branch(&self, name: &str) -> &Self {
        self.git(["branch", name]);
        self
    }

    /// Switches to branch `name`, creating it at HEAD when it does not exist.
    pub fn switch(&self, name: &str) -> &Self {
        let exists = Command::new("git")
            .env_clear()
            .envs(self.envs.iter().cloned())
            .current_dir(&self.dir)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ])
            .status()
            .expect("run git show-ref")
            .success();
        if exists {
            self.git(["switch", "--quiet", name]);
        } else {
            self.git(["switch", "--quiet", "-c", name]);
        }
        self
    }

    /// Points lightweight tag `name` at HEAD, moving it when it already exists.
    pub fn tag(&self, name: &str) -> &Self {
        self.git(["tag", "--force", name]);
        self
    }

    /// The sha of HEAD.
    pub fn head(&self) -> String {
        self.git(["rev-parse", "HEAD"])
    }

    /// Runs git in this repo; panics on failure and returns trimmed stdout.
    pub fn git<I, S>(&self, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        run_git(&self.envs, &self.dir, args)
    }
}

pub(crate) fn run_git<I, S>(envs: &[(OsString, OsString)], dir: &Path, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    cmd.env_clear()
        .envs(envs.iter().cloned())
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Test Upstream",
            "-c",
            "user.email=upstream@example.test",
        ])
        .args(args);
    let output = cmd.output().expect("run git");
    assert!(
        output.status.success(),
        "{cmd:?} failed in {}:\n{}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut stdout = String::from_utf8(output.stdout).expect("git output is UTF-8");
    if stdout.ends_with('\n') {
        stdout.pop();
    }
    stdout
}
