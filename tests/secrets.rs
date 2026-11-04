//! Secret detection (spec Rules, "No secrets"): reports with a fingerprint
//! instead of the matched text, and `[[secrets.allow]]` exemptions.
//!
//! Secret-like fixtures are built at runtime from pieces, so no literal token
//! is committed (GitHub push protection would block it).

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use support::Sandbox;

fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

fn write(sandbox: &Sandbox, rel: &str, contents: impl AsRef<[u8]>) -> PathBuf {
    let path = sandbox.skill_repo().join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, contents).unwrap();
    path
}

fn append(sandbox: &Sandbox, rel: &str, text: &str) {
    let path = sandbox.skill_repo().join(rel);
    let mut old = std::fs::read_to_string(&path).unwrap_or_default();
    old.push_str(text);
    std::fs::write(path, old).unwrap();
}

fn valid_skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Does {name} things\n---\n\n# {name}\n")
}

/// A skill repo with one own skill `pdf`.
fn repo() -> Sandbox {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(&sandbox, "skills/pdf/SKILL.md", valid_skill_md("pdf"));
    sandbox
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The spec's tree hash, computed independently of skiman.
fn expected_tree_hash(dir: &Path) -> String {
    fn files(root: &Path, dir: &Path, out: &mut Vec<(String, bool, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                files(root, &path, out);
            } else if meta.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                let exec = meta.permissions().mode() & 0o100 != 0;
                out.push((rel, exec, std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut list = Vec::new();
    files(dir, dir, &mut list);
    list.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut text = Vec::new();
    for (rel, exec, content) in list {
        text.extend_from_slice(rel.as_bytes());
        text.extend_from_slice(if exec { b"\x001\x00" } else { b"\x000\x00" });
        text.extend_from_slice(hex(&Sha256::digest(&content)).as_bytes());
        text.push(b'\n');
    }
    format!("sha256:{}", hex(&Sha256::digest(&text)))
}

/// Makes `skills/<name>` third-party: a `source` table, `upstream/<name>`
/// holding `upstream_files`, and a lock entry with its tree hash.
fn third_party(sandbox: &Sandbox, name: &str, upstream_files: &[(&str, &str)]) {
    for (rel, content) in upstream_files {
        write(sandbox, &format!("upstream/{name}/{rel}"), content);
    }
    append(
        sandbox,
        "skiman.toml",
        &format!(
            "\n[skills.{name}.source]\nurl = \"https://github.com/acme/skills.git\"\npath = \"{name}\"\n"
        ),
    );
    let hash = expected_tree_hash(&sandbox.skill_repo().join("upstream").join(name));
    append(
        sandbox,
        "skiman.lock",
        &format!(
            "version = 1\n\n[skills.{name}]\ncommit = \"{}\"\nsnapshot = \"{hash}\"\n",
            "a".repeat(40)
        ),
    );
}

fn fingerprint(matched: &str) -> String {
    format!("sha256:{}", hex(&Sha256::digest(matched.as_bytes())))
}

// Secret-like values, assembled so the source never holds one.
fn aws_key() -> String {
    ["AK", "IA", "Q2W3E4R5T6Y7U8I9"].concat()
}

/// What `skiman check` printed.
struct Checked {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(command: snapbox::cmd::Command) -> std::process::Output {
    command.output().unwrap()
}

/// Runs `skiman check` and asserts that none of `secrets` appears in its output.
fn check(sandbox: &Sandbox, secrets: &[&str]) -> Checked {
    check_with(sandbox, &[], secrets)
}

/// [`check`] with extra arguments (`--staged`).
fn check_with(sandbox: &Sandbox, args: &[&str], secrets: &[&str]) -> Checked {
    let output = run(sandbox.skiman().arg("check").args(args));
    let checked = Checked {
        code: output.status.code().unwrap(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    };
    for secret in secrets {
        assert!(
            !checked.stdout.contains(secret) && !checked.stderr.contains(secret),
            "matched text leaked into the output:\n{}{}",
            checked.stdout,
            checked.stderr
        );
    }
    checked
}

#[test]
fn a_secret_is_reported_with_path_line_rule_and_fingerprint_and_a_ready_to_paste_entry() {
    let sandbox = repo();
    let key = aws_key();
    write(
        &sandbox,
        "skills/pdf/tests/fixture.env",
        format!("# fixture\nAWS_ACCESS_KEY_ID={key}\n"),
    );

    let checked = check(&sandbox, &[&key]);
    let fp = fingerprint(&key);
    assert_eq!(checked.code, 1);
    assert_eq!(
        checked.stdout,
        format!(
            "skills/pdf/tests/fixture.env:2: aws-access-key: looks like a secret; fingerprint = \"{fp}\"\n\
             \n\
             If a match is not a real secret, add its entry to skiman.toml:\n\
             \n\
             [[secrets.allow]]\n\
             path = \"skills/pdf/tests/fixture.env\"\n\
             rule = \"aws-access-key\"\n\
             fingerprint = \"{fp}\"\n"
        )
    );
    assert_eq!(checked.stderr, "");
}

/// `(line, rule, fingerprint)` of every secret reported in `stdout`.
fn reported(stdout: &str, path: &str) -> Vec<(usize, String, String)> {
    stdout
        .lines()
        .filter_map(|l| l.strip_prefix(&format!("{path}:")))
        .filter_map(|rest| {
            let (line, rest) = rest.split_once(": ")?;
            let (rule, rest) = rest.split_once(": ")?;
            let fp = rest.strip_prefix("looks like a secret; fingerprint = \"")?;
            Some((
                line.parse().ok()?,
                rule.to_owned(),
                fp.trim_end_matches('"').to_owned(),
            ))
        })
        .collect()
}

/// Writes one file of `positives` (line, matched text) followed by
/// `negatives`, and asserts `check` reports exactly the positives under
/// `rule`, without printing any of the fixture text.
fn assert_detects(rule: &str, positives: &[(String, String)], negatives: &[String]) {
    let sandbox = repo();
    let mut text = String::new();
    for line in positives.iter().map(|(l, _)| l).chain(negatives) {
        text.push_str(line);
        text.push('\n');
    }
    write(&sandbox, "skills/pdf/fixture.txt", &text);
    let mut secrets: Vec<&str> = positives.iter().map(|(_, m)| m.as_str()).collect();
    secrets.extend(negatives.iter().map(String::as_str));

    let checked = check(&sandbox, &secrets);
    let expected: Vec<(usize, String, String)> = positives
        .iter()
        .enumerate()
        .map(|(i, (_, m))| (i + 1, rule.to_owned(), fingerprint(m)))
        .collect();
    assert_eq!(
        reported(&checked.stdout, "skills/pdf/fixture.txt"),
        expected,
        "{}",
        checked.stdout
    );
    assert_eq!(checked.code, if positives.is_empty() { 0 } else { 1 });
}

/// A line that is its own matched text.
fn whole(line: String) -> (String, String) {
    (line.clone(), line)
}

#[test]
fn private_key_detector() {
    let begin = |kind: &str| format!("-----{} {kind}{} KEY-----", "BEGIN", " PRIVATE");
    assert_detects(
        "private-key",
        &[
            whole(begin("RSA")),
            whole(begin("OPENSSH")),
            (format!("x{}y", begin("EC")), begin("EC")),
            whole(format!("-----BEGIN PRIVATE {}-----", "KEY")),
        ],
        &[
            format!("-----BEGIN PUBLIC {}-----", "KEY"),
            begin("rsa"),
            format!("-----BEGIN RSA PRIVATE {}----", "KEY"),
        ],
    );
}

#[test]
fn aws_access_key_detector() {
    let key = aws_key();
    assert_detects(
        "aws-access-key",
        &[whole(key.clone()), (format!("id = {key}9"), key.clone())],
        &[
            format!("{} end", &key[..19]),
            format!("{}q2w3e4r5t6y7u8i9", ["AK", "IA"].concat()),
        ],
    );
}

#[test]
fn github_token_detector() {
    let classic = format!("{}{}", "gh", "p_");
    let fine = format!("{}{}", "github", "_pat_");
    let alnum36 = "aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dE3fG5";
    assert_detects(
        "github-token",
        &[
            whole(format!("{classic}{alnum36}")),
            whole(format!("{fine}{}", "A1_b2C3_d4E5f6G7h8I9j0")),
        ],
        &[
            format!("{classic}{} end", &alnum36[..35]),
            format!("{fine}{} end", "A1_b2C3_d4E5f6G7h8I9j"),
        ],
    );
}

#[test]
fn openai_key_detector_needs_no_key_character_before_it_and_an_uppercase_letter_and_a_digit() {
    let sk = |rest: &str| format!("{}{rest}", "sk-");
    let body = "AbCdEfGh1234567890xyzQ";
    assert_detects(
        "openai-key",
        &[
            whole(sk(body)),
            (format!("key={}", sk(body)), sk(body)),
            (
                format!("\"{}\"", sk(&format!("proj-{body}"))),
                sk(&format!("proj-{body}")),
            ),
            (
                format!("x {}", sk(&format!("svcacct-{body}"))),
                sk(&format!("svcacct-{body}")),
            ),
            (
                format!("({})", sk(&format!("admin-{body}"))),
                sk(&format!("admin-{body}")),
            ),
        ],
        &[
            format!("task-{}", sk(body)),
            format!("a{}", sk(body)),
            format!("_{}", sk(body)),
            format!("9{}", sk(body)),
            sk("abcdefgh1234567890xyzq"),
            sk("AbCdEfGhIjKlMnOpQrStUv"),
            sk(&body[..19]),
        ],
    );
}

#[test]
fn slack_token_detector() {
    let xox = |c: char, rest: &str| format!("{}{c}-{rest}", ["xo", "x"].concat());
    assert_detects(
        "slack-token",
        &[
            whole(xox('b', "1234567890-abcDEF")),
            whole(xox('a', "1234567890")),
            whole(xox('p', "1234567890")),
            whole(xox('r', "1234567890")),
            whole(xox('s', "1234567890")),
        ],
        &[xox('c', "1234567890"), xox('b', "123456789")],
    );
}

#[test]
fn secret_assignment_detector_is_case_insensitive_and_skips_placeholders() {
    assert_detects(
        "secret-assignment",
        &[
            whole("password = \"hunter2hunter2\"".to_owned()),
            whole("TOKEN: 'abcdefgh12'".to_owned()),
            (
                "api_Secret=\"abcdefgh\"".to_owned(),
                "Secret=\"abcdefgh\"".to_owned(),
            ),
            // A match may start inside a skipped one.
            (
                "secret:\"$secret='abcdefghij'".to_owned(),
                "secret='abcdefghij'".to_owned(),
            ),
        ],
        &[
            "password = \"abcdefg\"".to_owned(),
            "password = hunter2hunter2".to_owned(),
            "token = \"$ABCDEFGHI\"".to_owned(),
            "token = \"<ABCDEFGHI>\"".to_owned(),
            "token = \"{ABCDEFGHI}\"".to_owned(),
            "token = \"%ABCDEFGHI%\"".to_owned(),
            "token = \"ABCexampleDEF\"".to_owned(),
            "token = \"ABCEXAMPLEDEF\"".to_owned(),
            "token = \"ABCyourDEF12\"".to_owned(),
            "token = \"ABCYourDEF12\"".to_owned(),
            "token = \"ABCxxxDEF12\"".to_owned(),
            "token = \"ABCXXXDEF12\"".to_owned(),
        ],
    );
}

/// The `[[secrets.allow]]` entries `check` printed for pasting.
fn printed_entries(stdout: &str) -> String {
    let (_, entries) = stdout
        .split_once("add its entry to skiman.toml:\n\n")
        .unwrap_or_else(|| panic!("no ready-to-paste entries in:\n{stdout}"));
    entries.to_owned()
}

#[test]
fn pasting_the_printed_entry_makes_check_pass_until_the_secret_changes() {
    let sandbox = repo();
    let key = aws_key();
    write(
        &sandbox,
        "skills/pdf/tests/fixture.env",
        format!("KEY={key}\n"),
    );
    let entries = printed_entries(&check(&sandbox, &[&key]).stdout);

    append(&sandbox, "skiman.toml", &format!("\n{entries}"));
    let checked = check(&sandbox, &[&key]);
    assert_eq!((checked.code, checked.stdout.as_str()), (0, ""));

    let changed = format!("{}X", &key[..19]);
    write(
        &sandbox,
        "skills/pdf/tests/fixture.env",
        format!("KEY={changed}\n"),
    );
    let checked = check(&sandbox, &[&key, &changed]);
    assert_eq!(checked.code, 1);
    assert_eq!(
        reported(&checked.stdout, "skills/pdf/tests/fixture.env"),
        [(1, "aws-access-key".to_owned(), fingerprint(&changed))]
    );
}

#[test]
fn one_entry_covers_the_skill_its_upstream_snapshot_and_its_conflict_records() {
    let sandbox = repo();
    let key = aws_key();
    let fixture = format!("KEY={key}\n");
    let copies = [
        "skills/pdf/tests/fixture.env",
        "skills/pdf/tests/fixture.env.skiman-upstream",
        "skills/pdf/.skiman-conflicts/tests/fixture.env.skiman-upstream",
        "skills/pdf/tests.skiman-upstream/fixture.env",
        "skills/pdf/.skiman-conflicts/tests.skiman-upstream/fixture.env",
    ];
    for path in copies {
        write(&sandbox, path, &fixture);
    }
    third_party(
        &sandbox,
        "pdf",
        &[
            ("SKILL.md", &valid_skill_md("pdf")),
            ("tests/fixture.env", &fixture),
        ],
    );

    let checked = check(&sandbox, &[&key]);
    let mut paths: Vec<&str> = copies.to_vec();
    paths.push("upstream/pdf/tests/fixture.env");
    for path in &paths {
        assert_eq!(
            reported(&checked.stdout, path).len(),
            1,
            "{path}:\n{}",
            checked.stdout
        );
    }
    let entries = printed_entries(&checked.stdout);
    assert_eq!(
        entries,
        format!(
            "[[secrets.allow]]\npath = \"skills/pdf/tests/fixture.env\"\nrule = \"aws-access-key\"\nfingerprint = \"{}\"\n",
            fingerprint(&key)
        )
    );

    append(&sandbox, "skiman.toml", &format!("\n{entries}"));
    let checked = check(&sandbox, &[&key]);
    // Only the conflict records themselves remain.
    assert!(
        checked
            .stdout
            .lines()
            .all(|l| l.contains(": unresolved-conflict: ")),
        "{}",
        checked.stdout
    );
    assert!(!checked.stdout.is_empty());
}

#[test]
fn a_match_only_in_the_snapshot_gets_a_skill_path_entry_and_one_in_instructions_its_own_path() {
    let sandbox = repo();
    let key = aws_key();
    third_party(
        &sandbox,
        "pdf",
        &[
            ("SKILL.md", &valid_skill_md("pdf")),
            ("old/notes.txt", &format!("{key}\n")),
        ],
    );
    write(
        &sandbox,
        "instructions/shared.md",
        format!("# Shared\n\nkey {key}\n"),
    );

    let checked = check(&sandbox, &[&key]);
    let fp = fingerprint(&key);
    assert_eq!(
        reported(&checked.stdout, "upstream/pdf/old/notes.txt"),
        [(1, "aws-access-key".to_owned(), fp.clone())]
    );
    assert_eq!(
        reported(&checked.stdout, "instructions/shared.md"),
        [(3, "aws-access-key".to_owned(), fp.clone())]
    );
    let entries = printed_entries(&checked.stdout);
    assert_eq!(
        entries,
        format!(
            "[[secrets.allow]]\npath = \"instructions/shared.md\"\nrule = \"aws-access-key\"\nfingerprint = \"{fp}\"\n\n\
             [[secrets.allow]]\npath = \"skills/pdf/old/notes.txt\"\nrule = \"aws-access-key\"\nfingerprint = \"{fp}\"\n"
        )
    );

    append(&sandbox, "skiman.toml", &format!("\n{entries}"));
    let checked = check(&sandbox, &[&key]);
    assert_eq!((checked.code, checked.stdout.as_str()), (0, ""));
}

#[test]
fn an_unused_entry_is_listed_and_check_still_exits_0() {
    let sandbox = repo();
    append(
        &sandbox,
        "skiman.toml",
        "\n[[secrets.allow]]\npath = \"skills/pdf/gone.env\"\nrule = \"aws-access-key\"\nfingerprint = \"sha256:00\"\n",
    );
    let line = std::fs::read_to_string(sandbox.skill_repo().join("skiman.toml"))
        .unwrap()
        .lines()
        .position(|l| l == "[[secrets.allow]]")
        .unwrap()
        + 1;

    let checked = check(&sandbox, &[]);
    assert_eq!(checked.code, 0);
    assert_eq!(
        checked.stdout,
        format!(
            "skiman.toml:{line}: unused secret exemption: no file it covers has a `aws-access-key` match \
             with fingerprint sha256:00 (path = \"skills/pdf/gone.env\"); delete the entry\n"
        )
    );
}

#[test]
fn new_refuses_a_description_that_contains_a_secret() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let key = aws_key();
    let assignment = "password='hunter2hunter2'";

    for (description, rule, matched) in [
        (
            format!("Uses {key} to sign"),
            "aws-access-key",
            key.as_str(),
        ),
        (
            format!("Logs in with {assignment}"),
            "secret-assignment",
            assignment,
        ),
    ] {
        let output = run(sandbox.skiman().args(["new", "pdf", "-d", &description]));
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(
            !stdout.contains(matched) && !stderr.contains(matched),
            "{stdout}{stderr}"
        );
        assert_eq!(stdout, "");
        let fp = fingerprint(matched);
        assert_eq!(
            stderr,
            format!(
                "error: refusing to write content that breaks the rules:\n  \
                 skills/pdf/SKILL.md:3: {rule}: looks like a secret; fingerprint = \"{fp}\"\n\
                 \n\
                 If a match is not a real secret, add its entry to skiman.toml:\n\
                 \n\
                 [[secrets.allow]]\n\
                 path = \"skills/pdf/SKILL.md\"\n\
                 rule = \"{rule}\"\n\
                 fingerprint = \"{fp}\"\n"
            )
        );
        assert!(!sandbox.skill_repo().join("skills/pdf").exists());
    }
}

#[test]
fn pre_commit_rejects_a_staged_secret_until_the_printed_entry_is_added() {
    let sandbox = repo();
    let key = aws_key();
    let repo = sandbox.skill_repo();
    write(
        &sandbox,
        "skills/pdf/tests/fixture.env",
        format!("KEY={key}\n"),
    );
    sandbox.git(&repo, ["add", "--all"]);
    // Only the staged snapshot holds the secret.
    write(&sandbox, "skills/pdf/tests/fixture.env", "KEY=\n");
    let commit = || {
        run(sandbox.command("git").current_dir(&repo).args([
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=user@example.test",
            "commit",
            "--quiet",
            "-m",
            "add pdf",
        ]))
    };

    // git shows the pre-commit hook's stdout on stderr.
    let rejected = commit();
    let stderr = String::from_utf8(rejected.stderr).unwrap();
    assert_eq!(rejected.status.code(), Some(1), "{stderr}");
    assert!(!stderr.contains(&key), "matched text leaked:\n{stderr}");
    let entries = printed_entries(&stderr);

    append(&sandbox, "skiman.toml", &format!("\n{entries}"));
    sandbox.git(&repo, ["add", "skiman.toml"]);
    let accepted = commit();
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_eq!(
        sandbox.git(&repo, ["show", "HEAD:skills/pdf/tests/fixture.env"]),
        format!("KEY={key}")
    );
}

#[test]
fn check_and_check_staged_apply_the_secret_and_shared_body_rules_to_their_own_tree() {
    let sandbox = repo();
    let key = aws_key();
    let repo = sandbox.skill_repo();
    // About 83 % of Antigravity's 24000-byte limit, with a secret on line 3
    // and an `@` import on line 4.
    let mut body = format!("# Shared\n\nkey {key}\n@imports/rules.md\n");
    body.push_str(&("x".repeat(99) + "\n").repeat(200));
    write(&sandbox, "instructions/shared.md", &body);
    sandbox.git(&repo, ["add", "--all"]);
    // The working tree has a clean shared body and an unused entry; neither is staged.
    write(&sandbox, "instructions/shared.md", "# Shared\n");
    append(
        &sandbox,
        "skiman.toml",
        "\n[[secrets.allow]]\npath = \"skills/pdf/gone.env\"\nrule = \"aws-access-key\"\nfingerprint = \"sha256:00\"\n",
    );

    let checked = check(&sandbox, &[&key]);
    assert_eq!(checked.code, 0, "{}", checked.stdout);
    assert!(
        checked.stdout.contains(": unused secret exemption: ")
            && checked.stdout.lines().count() == 1,
        "{}",
        checked.stdout
    );
    assert_eq!(checked.stderr, "");

    let staged = check_with(&sandbox, &["--staged"], &[&key]);
    assert_eq!(staged.code, 1, "{}", staged.stdout);
    assert_eq!(
        reported(&staged.stdout, "instructions/shared.md"),
        [(3, "aws-access-key".to_owned(), fingerprint(&key))]
    );
    assert!(
        staged
            .stdout
            .contains("instructions/shared.md:4: at-import: `@imports/rules.md` "),
        "{}",
        staged.stdout
    );
    assert!(
        !staged.stdout.contains("unused secret exemption"),
        "{}",
        staged.stdout
    );
    assert!(printed_entries(&staged.stdout).contains("path = \"instructions/shared.md\""));
    assert!(
        staged.stderr.starts_with(&format!(
            "warning: instructions/shared.md: shared-body-size: {} bytes is at least 80 % of the limit of 24000 bytes",
            body.len()
        )),
        "{}",
        staged.stderr
    );
}
