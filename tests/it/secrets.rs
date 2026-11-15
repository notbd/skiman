//! Secret detection (spec Rules, "No secrets"): reports with a fingerprint
//! instead of the matched text, and `[[secrets.allow]]` exemptions.
//!
//! Secret-like fixtures are built at runtime from pieces, so no literal token
//! is committed (GitHub push protection would block it).

use std::path::PathBuf;
use std::time::Duration;

use crate::support::{Sandbox, aws_key, fingerprint, init, skill_md, tree_hash};

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

/// The one warning `skiman check` prints for [`repo`]: `pdf` is in no group folder.
const NO_GROUP: &str = "warning: skills/pdf: skill-info: it is in no group folder; move it into one with `skiman mv pdf --group <group>`\n";

/// A skill repo with one own skill `pdf`.
fn repo() -> Sandbox {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(&sandbox, "skills/pdf/SKILL.md", skill_md("pdf"));
    // What a person reads about the skill, so `skiman check` warns of its group only.
    append(
        &sandbox,
        "skiman.toml",
        "\n[skills.pdf]\nsummary = \"Fill PDF forms\"\norigin = \"written here\"\n",
    );
    sandbox
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
    let hash = tree_hash(&sandbox.skill_repo().join("upstream").join(name));
    append(
        sandbox,
        "skiman.lock",
        &format!(
            "version = 1\n\n[skills.{name}]\ncommit = \"{}\"\nsnapshot = \"{hash}\"\n",
            "a".repeat(40)
        ),
    );
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
    assert_eq!(checked.stderr, NO_GROUP);
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

fn pem_begin(label: &str) -> String {
    format!("-----{} {label}{} KEY-----", "BEGIN", "PRIVATE")
}

fn pem_end(label: &str) -> String {
    format!("-----END {label}PRIVATE KEY-----")
}

/// A private key block (`label` is `"RSA "`, `"EC "`, `""`, …): its BEGIN
/// line, `body` and its END line joined by `eol`, with no `eol` at the end.
fn pem_block(label: &str, body: &[&str], eol: &str) -> String {
    let mut lines = vec![pem_begin(label)];
    lines.extend(body.iter().map(|l| (*l).to_owned()));
    lines.push(pem_end(label));
    lines.join(eol)
}

const PEM_BODY: [&str; 2] = [
    "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2",
    "d3h5ejAxMjM0NTY3ODkrL0FCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFla",
];

/// Writes `text` as one file and asserts `check` reports exactly
/// `expected` (line, fingerprinted text) under `private-key`.
fn assert_private_keys(text: &str, expected: &[(usize, &str)]) {
    let sandbox = repo();
    write(&sandbox, "skills/pdf/keys.txt", text);
    let secrets: Vec<&str> = expected.iter().map(|(_, t)| *t).collect();

    let checked = check(&sandbox, &secrets);
    let expected: Vec<(usize, String, String)> = expected
        .iter()
        .map(|(line, t)| (*line, "private-key".to_owned(), fingerprint(t)))
        .collect();
    assert_eq!(
        reported(&checked.stdout, "skills/pdf/keys.txt"),
        expected,
        "{}",
        checked.stdout
    );
}

#[test]
fn a_lone_private_key_header_is_fingerprinted_as_the_header_and_a_whole_block_from_begin_to_end() {
    let header = pem_begin("RSA ");
    assert_private_keys(&format!("{header}\n"), &[(1, &header)]);

    let block = pem_block("RSA ", &PEM_BODY, "\n");
    assert_private_keys(&format!("Key:\n{block}\nDone.\n"), &[(2, &block)]);

    let changed = block.replacen("QUJD", "QUJE", 1);
    assert_ne!(fingerprint(&changed), fingerprint(&block));
    assert_private_keys(&format!("Key:\n{changed}\nDone.\n"), &[(2, &changed)]);
}

#[test]
fn an_entry_for_an_example_header_does_not_cover_a_whole_key_pasted_into_the_same_file() {
    let sandbox = repo();
    let header = pem_begin("RSA ");
    write(
        &sandbox,
        "skills/pdf/notes.md",
        format!("A key starts with:\n\n{header}\n"),
    );
    let entries = printed_entries(&check(&sandbox, &[&header]).stdout);
    append(&sandbox, "skiman.toml", &format!("\n{entries}"));
    assert_eq!(check(&sandbox, &[&header]).code, 0);

    let block = pem_block("RSA ", &PEM_BODY, "\n");
    append(&sandbox, "skills/pdf/notes.md", &format!("\n{block}\n"));

    let checked = check(&sandbox, &[&block]);
    assert_eq!(checked.code, 1, "{}", checked.stdout);
    assert_eq!(
        reported(&checked.stdout, "skills/pdf/notes.md"),
        [(5, "private-key".to_owned(), fingerprint(&block))]
    );
}

#[test]
fn private_key_blocks_and_headers_are_each_reported_once_and_never_swallow_each_other() {
    let block = pem_block("RSA ", &PEM_BODY, "\n");
    let header = pem_begin("RSA ");
    // Two whole blocks in a row.
    assert_private_keys(&format!("{block}\n{block}\n"), &[(1, &block), (5, &block)]);
    // An example header a few lines above a whole block.
    assert_private_keys(
        &format!("{header}\nsome\ntext\n{block}\n"),
        &[(1, &header), (4, &block)],
    );
    // A block without its END line before a whole block.
    assert_private_keys(
        &format!("{header}\n{}\n{block}\n", PEM_BODY[0]),
        &[(1, &header), (3, &block)],
    );
    // A BEGIN right after an END on the same line.
    assert_private_keys(&format!("{block}{block}\n"), &[(1, &block), (4, &block)]);
}

#[test]
fn a_block_whose_first_end_line_has_another_label_keeps_the_header_fingerprint() {
    let header = pem_begin("RSA ");
    assert_private_keys(
        &format!("{header}\n{}\n{}\n", PEM_BODY[0], pem_end("EC ")),
        &[(1, &header)],
    );
}

#[test]
fn a_block_with_encryption_headers_is_fingerprinted_whole() {
    let block = pem_block(
        "RSA ",
        &[
            "Proc-Type: 4,ENCRYPTED",
            "DEK-Info: AES-256-CBC,0123456789ABCDEF0123456789ABCDEF",
            "",
            PEM_BODY[0],
            PEM_BODY[1],
        ],
        "\n",
    );
    assert_private_keys(&format!("{block}\n"), &[(1, &block)]);
}

#[test]
fn a_block_is_fingerprinted_up_to_its_end_line_with_lf_crlf_or_no_final_newline() {
    let lf = pem_block("", &PEM_BODY, "\n");
    assert_private_keys(&format!("{lf}\n"), &[(1, &lf)]);
    let crlf = pem_block("", &PEM_BODY, "\r\n");
    assert_private_keys(&format!("x\r\n{crlf}\r\nafter\r\n"), &[(2, &crlf)]);
    let open = pem_block("OPENSSH ", &PEM_BODY, "\n");
    assert_private_keys(&open, &[(1, &open)]);
}

/// Five times the slowest debug run of the tests below on a busy machine
/// (under 10 s), and far below a scan that rescans the file per match
/// (over 300 s).
const SCAN_TIMEOUT: Duration = Duration::from_secs(50);

#[test]
fn check_scans_a_long_run_of_rejected_openai_key_prefixes_in_linear_time() {
    let sandbox = repo();
    write(&sandbox, "skills/pdf/blob.txt", "sk-".repeat(20_000));

    sandbox
        .skiman()
        .arg("check")
        .timeout(SCAN_TIMEOUT)
        .assert()
        .code(0)
        .stdout_eq("");
}

#[test]
fn check_numbers_the_lines_of_many_matches_in_linear_time() {
    let sandbox = repo();
    let header = format!("-----{} RSA{} KEY-----\n", "BEGIN", " PRIVATE");
    write(&sandbox, "skills/pdf/headers.txt", header.repeat(40_000));

    let output = run(sandbox.skiman().arg("check").timeout(SCAN_TIMEOUT));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    let lines: Vec<usize> = reported(&stdout, "skills/pdf/headers.txt")
        .into_iter()
        .map(|(line, _, _)| line)
        .collect();
    assert_eq!(lines, (1..=40_000).collect::<Vec<_>>());
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
    let pw = ["pass", "word"].concat();
    assert_detects(
        "secret-assignment",
        &[
            whole(format!("{pw} = \"hunter2hunter2\"")),
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
            format!("{pw} = \"abcdefg\""),
            format!("{pw} = hunter2hunter2"),
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
            ("SKILL.md", &skill_md("pdf")),
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
            ("SKILL.md", &skill_md("pdf")),
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
    let assignment = format!("{}='hunter2hunter2'", ["pass", "word"].concat());

    for (description, rule, matched) in [
        (
            format!("Uses {key} to sign"),
            "aws-access-key",
            key.as_str(),
        ),
        (
            format!("Logs in with {assignment}"),
            "secret-assignment",
            assignment.as_str(),
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
    assert_eq!(checked.stderr, NO_GROUP);

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
