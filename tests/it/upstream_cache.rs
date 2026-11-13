//! The git cache of an upstream under machine state
//! (`<state>/skiman/cache/<sha256 of the URL>.git`): fetches into one cache
//! take turns, and a fetch killed part-way does not stop the next one.

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::support::{Sandbox, Upstream, hex, init, skill_md, write};

/// `acme/pdf` with one skill at its root, committed.
fn pdf(sandbox: &Sandbox) -> Upstream {
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", skill_md("pdf"));
    up.commit("v1");
    up
}

fn cache(sandbox: &Sandbox, url: &str) -> PathBuf {
    let name = hex(&Sha256::digest(url.as_bytes()));
    sandbox
        .state_home()
        .join(format!("skiman/cache/{name}.git"))
}

/// The first line `child` writes to its piped stderr; kills it and fails
/// when there is none within 30 seconds.
fn first_stderr_line(child: &mut Child) -> String {
    let stderr = child.stderr.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stderr).read_line(&mut line);
        let _ = send.send(line);
    });
    receive
        .recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| {
            let _ = child.kill();
            panic!("no line on stderr within 30 seconds")
        })
}

#[test]
fn an_add_waits_while_another_skiman_fetches_into_the_same_cache() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = pdf(&sandbox);
    let lock_file = cache(&sandbox, &up.url()).with_extension("git.lock");
    std::fs::create_dir_all(lock_file.parent().unwrap()).unwrap();
    let lock = std::fs::File::create(&lock_file).unwrap();
    lock.lock().unwrap();
    // The user's default ref format, set both ways, is not the cache's.
    sandbox.git(
        sandbox.root(),
        ["config", "--global", "init.defaultRefFormat", "reftable"],
    );

    let mut add = sandbox
        .std_command(env!("CARGO_BIN_EXE_skiman"))
        .args(["add", "acme/pdf", "--yes"])
        .env("GIT_DEFAULT_REF_FORMAT", "reftable")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let first = first_stderr_line(&mut add);
    drop(lock);
    let status = add.wait().unwrap();

    assert_eq!(
        first,
        format!(
            "waiting for another skiman, or a git it started, to finish fetching from https://github.com/acme/pdf.git (it holds {})\n",
            lock_file.display()
        )
    );
    assert!(status.success(), "{status}");
    assert!(sandbox.skill_repo().join("skills/pdf/SKILL.md").is_file());
    let cache = cache(&sandbox, &up.url());
    assert_eq!(
        sandbox.git(
            &cache,
            [
                "config",
                "--default",
                "files",
                "--get",
                "extensions.refStorage"
            ]
        ),
        "files"
    );
}

/// Creates its file when dropped, so a fetch held on it never outlives the test.
struct Release(PathBuf);

impl Drop for Release {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "");
    }
}

#[test]
fn a_fetch_left_running_by_a_killed_skiman_keeps_the_cache_locked() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = pdf(&sandbox);
    let url = format!("ssh://fake{}", up.path().display());
    // An ssh that runs the remote command here. With `HOLD` set, the second
    // connection (the fetch, after `ls-remote`) waits for `$HOLD/release`,
    // or until the sandbox is gone.
    let ssh = sandbox.root().join("ssh");
    write(
        &ssh,
        "#!/bin/sh\nfor last; do :; done\nif [ -n \"$HOLD\" ]; then\n  n=$(($(cat \"$HOLD/count\" 2> /dev/null || echo 0) + 1))\n  echo $n > \"$HOLD/count\"\n  if [ $n -eq 2 ]; then\n    touch \"$HOLD/started\"\n    while [ -d \"$HOLD\" ] && [ ! -e \"$HOLD/release\" ]; do sleep 0.05; done\n  fi\nfi\nexec sh -c \"$last\"\n",
    );
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let hold = sandbox.root().join("hold");
    std::fs::create_dir_all(&hold).unwrap();
    let release = Release(hold.join("release"));
    let skiman = |args: &[&str]| {
        let mut cmd = sandbox.std_command(env!("CARGO_BIN_EXE_skiman"));
        cmd.args(args)
            .env("GIT_SSH_COMMAND", &ssh)
            .env("GIT_SSH_VARIANT", "simple")
            .stdout(Stdio::null());
        cmd
    };

    let mut first = skiman(&["add", &url, "--dry-run"])
        .env("HOLD", &hold)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(&hold.join("started"));
    // Only skiman is killed: the git fetch it started goes on.
    first.kill().unwrap();
    first.wait().unwrap();
    let mut second = skiman(&["add", &url, "--yes"])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let line = first_stderr_line(&mut second);
    drop(release);
    let status = second.wait().unwrap();

    assert_eq!(
        line,
        format!(
            "waiting for another skiman, or a git it started, to finish fetching from {url} (it holds {})\n",
            cache(&sandbox, &url).with_extension("git.lock").display()
        )
    );
    assert!(status.success(), "{status}");
    assert!(sandbox.skill_repo().join("skills/pdf/SKILL.md").is_file());
    assert!(!cache(&sandbox, &url).join("shallow.lock").exists());
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_fetch_killed_after_the_commit_object_arrived_does_not_stop_the_next_add() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = pdf(&sandbox);
    // What a git fetch killed part-way leaves: its lock file, and the commit
    // object without the tree it names.
    let cache = cache(&sandbox, &up.url());
    sandbox.git(
        sandbox.root(),
        ["init", "--quiet", "--bare", cache.to_str().unwrap()],
    );
    let commit = up.head();
    let object = sandbox.root().join("commit-object");
    std::fs::write(&object, up.git(["cat-file", "commit", &commit]) + "\n").unwrap();
    let stored = sandbox.git(
        &cache,
        [
            "hash-object",
            "-t",
            "commit",
            "-w",
            object.to_str().unwrap(),
        ],
    );
    assert_eq!(stored, commit);
    std::fs::write(cache.join("shallow.lock"), "").unwrap();

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);
    assert!(sandbox.skill_repo().join("skills/pdf/SKILL.md").is_file());
    assert!(!cache.join("shallow.lock").exists());
}
