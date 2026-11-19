//! The locks a write holds for its whole run (spec Distribution and
//! refresh), taken in this order: `<state dir>/lock`, one per machine,
//! because the user config, the tool directories and the backups belong to
//! the machine; then `<repo>/.skiman/lock`, because two state folders (two
//! `XDG_STATE_HOME`s) can share one canon.

use std::fs::{File, TryLockError};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::canon::Canon;
use crate::failure::Failure;
use crate::paths::Paths;

/// How long a command waits for busy locks before failing with exit 4.
pub const WAIT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

/// Held until dropped; the operating system releases it when the process ends.
#[derive(Debug)]
pub struct CanonLock {
    _machine: File,
    _repo: File,
}

pub enum Acquire {
    /// Wait up to [`WAIT`], then fail with exit 4.
    Wait,
    /// Try once; `Ok(None)` when a lock is busy (`sync --hook`).
    TryOnce,
}

impl CanonLock {
    /// Takes both locks, creating their folders and files when missing.
    /// Refuses (exit 4) when, by the time the machine lock is taken, the user
    /// config names another canon: a command switched it meanwhile.
    pub fn acquire(
        paths: &Paths,
        repo: &Canon,
        how: Acquire,
    ) -> Result<Option<CanonLock>, Failure> {
        let deadline = Instant::now() + WAIT;
        let Some(machine) = take(&paths.state_dir().join("lock"), &how, deadline)? else {
            return Ok(None);
        };
        let configured = Canon::from_config(paths)?;
        if configured.root() != repo.root() {
            return Err(Failure::operational(format!(
                "the canon changed to {} while this command waited; nothing was changed, run it again",
                configured.root().display()
            )));
        }
        let Some(file) = take(&repo.canon_lock_file(), &how, deadline)? else {
            return Ok(None);
        };
        Ok(Some(CanonLock {
            _machine: machine,
            _repo: file,
        }))
    }

    /// Takes both locks, waiting up to [`WAIT`].
    pub fn wait(paths: &Paths, repo: &Canon) -> Result<CanonLock, Failure> {
        Ok(CanonLock::acquire(paths, repo, Acquire::Wait)?.expect("waiting never returns busy"))
    }

    /// The machine lock alone, for `init`: it decides what to do with a folder
    /// and which canon the user config names, so two inits wait for
    /// each other; [`CanonLock::of_init`] adds the repo lock.
    pub fn machine(paths: &Paths) -> Result<File, Failure> {
        let deadline = Instant::now() + WAIT;
        Ok(take(&paths.state_dir().join("lock"), &Acquire::Wait, deadline)?.expect("waits"))
    }

    /// `init`'s locks: `machine` from [`CanonLock::machine`], then the repo lock.
    pub fn of_init(machine: File, repo: &Canon) -> Result<CanonLock, Failure> {
        let deadline = Instant::now() + WAIT;
        let file = take(&repo.canon_lock_file(), &Acquire::Wait, deadline)?.expect("waits");
        Ok(CanonLock {
            _machine: machine,
            _repo: file,
        })
    }

    /// Whether another process holds the repo lock. Read-only: a missing
    /// lock file means nobody holds it, and it is never created; when the
    /// lock is free, it is taken and released at once.
    pub fn is_held(repo: &Canon) -> std::io::Result<bool> {
        let file = match File::open(repo.canon_lock_file()) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        match file.try_lock() {
            Ok(()) => Ok(false),
            Err(TryLockError::WouldBlock) => Ok(true),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }
}

/// The lock on `path`, created with its folder when missing; `None` when it
/// is busy and `how` tries once.
fn take(path: &Path, how: &Acquire, deadline: Instant) -> Result<Option<File>, Failure> {
    let io_failure =
        |e: std::io::Error| Failure::operational(format!("cannot lock {}: {e}", path.display()));
    std::fs::create_dir_all(path.parent().expect("a lock file has a parent"))
        .map_err(io_failure)?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(io_failure)?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
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
