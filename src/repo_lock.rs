//! The repo lock: an exclusive lock on `<repo>/.skiman/lock`, held by
//! mutating commands and sync for their whole run (spec Distribution and refresh).

use std::fs::{File, TryLockError};
use std::time::{Duration, Instant};

use crate::failure::Failure;
use crate::repo::SkillRepo;

/// How long a command waits for a busy lock before failing with exit 4.
pub const WAIT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

/// Held until dropped; the operating system releases it when the process ends.
#[derive(Debug)]
pub struct RepoLock {
    _file: File,
}

pub enum Acquire {
    /// Wait up to [`WAIT`], then fail with exit 4.
    Wait,
    /// Try once; `Ok(None)` when the lock is busy (`sync --hook`).
    TryOnce,
}

impl RepoLock {
    /// Takes the lock, creating `.skiman/` and the lock file when missing.
    pub fn acquire(repo: &SkillRepo, how: Acquire) -> Result<Option<RepoLock>, Failure> {
        let path = repo.repo_lock_file();
        let io_failure = |e: std::io::Error| {
            Failure::operational(format!("cannot lock {}: {e}", path.display()))
        };
        std::fs::create_dir_all(repo.local_dir()).map_err(io_failure)?;
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(io_failure)?;
        let deadline = Instant::now() + WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Some(RepoLock { _file: file })),
                Err(TryLockError::Error(e)) => return Err(io_failure(e)),
                Err(TryLockError::WouldBlock) => match how {
                    Acquire::TryOnce => return Ok(None),
                    Acquire::Wait if Instant::now() >= deadline => {
                        return Err(Failure::operational(format!(
                            "another skiman command holds {} (waited {} seconds)",
                            path.display(),
                            WAIT.as_secs()
                        )));
                    }
                    Acquire::Wait => std::thread::sleep(POLL),
                },
            }
        }
    }

    /// Takes the lock, waiting up to [`WAIT`].
    pub fn wait(repo: &SkillRepo) -> Result<RepoLock, Failure> {
        Ok(RepoLock::acquire(repo, Acquire::Wait)?.expect("waiting never returns busy"))
    }
}
