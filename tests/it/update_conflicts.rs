//! `skiman update` with conflicts (spec Merge semantics): markers with the
//! fixed labels, conflict records, exit 3, and the refusals and safety
//! checks around them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::support::{Sandbox, Upstream, tree};

fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

fn read(path: impl AsRef<Path>) -> Vec<u8> {
    std::fs::read(path.as_ref()).unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Twenty numbered lines, so git sees edits far apart as separate hunks and
/// detects renames.
fn lines(edits: &[(usize, &str)]) -> String {
    (1..=20)
        .map(|n| {
            let line = edits
                .iter()
                .find(|(at, _)| *at == n)
                .map_or_else(|| format!("line {n}"), |(_, text)| (*text).to_owned());
            format!("{line}\n")
        })
        .collect()
}

const PDF_SKILL_MD: &str = "---\nname: pdf\ndescription: Fill PDF forms\n---\n\n# pdf\n";

/// Upstream `acme/pdf` holding `SKILL.md` plus `files`, installed as `pdf`.
fn installed_with(sandbox: &Sandbox, files: &[(&str, &[u8])]) -> Upstream {
    init(sandbox);
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", PDF_SKILL_MD);
    for (path, contents) in files {
        up.write(path, contents);
    }
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);
    up
}

fn installed(sandbox: &Sandbox) -> Upstream {
    installed_with(sandbox, &[("reference.md", lines(&[]).as_bytes())])
}

fn skill(sandbox: &Sandbox) -> PathBuf {
    sandbox.skill_repo().join("skills/pdf")
}

/// The whole skill repo (including `.git` and `.skiman`).
fn skill_repo_tree(sandbox: &Sandbox) -> BTreeMap<PathBuf, String> {
    tree(sandbox.skill_repo())
}

fn text_conflict(local: &str, upstream: &str) -> String {
    let mut text = lines(&[])
        .lines()
        .take(4)
        .map(|l| format!("{l}\n"))
        .collect::<String>();
    text.push_str(&format!(
        "<<<<<<< local (skiman)\n{local}\n=======\n{upstream}\n>>>>>>> upstream (skiman)\n"
    ));
    text.extend((6..=20).map(|n| format!("line {n}\n")));
    text
}

#[test]
fn a_text_conflict_is_written_with_the_fixed_labels_and_exits_3() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    write(skill(&sandbox).join("reference.md"), lines(&[(5, "mine")]));
    up.write("reference.md", lines(&[(5, "theirs")]));
    let commit = up.commit("v2");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(3)
        .stdout_eq(format!(
            "skill pdf: updated to {commit} with conflicts; changed files:\n\
             \x20 modified reference.md\n\
             saved the old folders and records of pdf to [..]\n\
             skill pdf: resolve these conflicts in skills/pdf, then delete the conflict records:\n\
             \x20 reference.md: conflict markers\n"
        ))
        .stderr_eq("");

    assert_eq!(
        String::from_utf8(read(skill(&sandbox).join("reference.md"))).unwrap(),
        text_conflict("mine", "theirs")
    );
}

/// Every regular file below `dir`, by relative path.
fn files(dir: impl AsRef<Path>) -> BTreeMap<String, Vec<u8>> {
    tree(dir.as_ref())
        .into_iter()
        .filter(|(_, v)| v.starts_with("file "))
        .map(|(p, _)| {
            let bytes = read(dir.as_ref().join(&p));
            (p.to_str().unwrap().to_owned(), bytes)
        })
        .collect()
}

/// A change to one side: `Some` writes the file, `None` deletes it.
type Edit<'a> = (&'a str, Option<&'a [u8]>);

/// Installs `base`, applies `ours` to `skills/pdf` and `theirs` to the
/// upstream, runs `skiman update pdf` and asserts exit 3. Returns stdout
/// and the files of the skill.
fn conflicting_update(
    base: &[(&str, &[u8])],
    ours: &[Edit],
    theirs: &[Edit],
) -> (String, BTreeMap<String, Vec<u8>>) {
    let sandbox = Sandbox::new();
    let up = installed_with(&sandbox, base);
    let skill = skill(&sandbox);
    for (path, contents) in ours {
        match contents {
            Some(c) => write(skill.join(path), c),
            None => std::fs::remove_file(skill.join(path)).unwrap(),
        }
    }
    for (path, contents) in theirs {
        match contents {
            Some(c) => up.write(path, c),
            None => up.remove(path),
        };
    }
    up.commit("v2");
    let assert = sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(3)
        .stderr_eq("");
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let mut files = files(&skill);
    assert_eq!(
        files.remove("SKILL.md").as_deref(),
        Some(PDF_SKILL_MD.as_bytes())
    );
    (stdout, files)
}

fn conflict_list(stdout: &str) -> Vec<&str> {
    stdout
        .split_once("then delete the conflict records:\n")
        .unwrap_or_else(|| panic!("no conflict list in:\n{stdout}"))
        .1
        .lines()
        .collect()
}

fn owned(files: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| ((*p).to_owned(), c.to_vec()))
        .collect()
}

/// The spec's tree hash of the regular files below `dir`, computed here
/// independently of skiman.
fn tree_hash(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let hex = |bytes: &[u8]| -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() };
    let mut text = Vec::new();
    for (path, value) in tree(dir) {
        let Some(rest) = value.strip_prefix("file ") else {
            continue;
        };
        let (mode, content) = rest.split_once(' ').unwrap();
        let exec = u32::from_str_radix(mode, 8).unwrap() & 0o100 != 0;
        text.extend_from_slice(path.to_str().unwrap().as_bytes());
        text.extend_from_slice(if exec { b"\x001\x00" } else { b"\x000\x00" });
        text.extend_from_slice(hex(&Sha256::digest(content.as_bytes())).as_bytes());
        text.push(b'\n');
    }
    format!("sha256:{}", hex(&Sha256::digest(&text)))
}

/// `pdf` from an upstream whose root holds a `.gitignore`, laid out the way
/// skiman installed it before it left such files out: the file in
/// `skills/pdf` and `upstream/pdf`, and in the lock's snapshot hash.
fn installed_with_a_gitignore(sandbox: &Sandbox) -> Upstream {
    let up = installed_with(sandbox, &[(".gitignore", b"*.log\n")]);
    let repo = sandbox.skill_repo();
    for dir in ["skills/pdf", "upstream/pdf"] {
        write(repo.join(dir).join(".gitignore"), "*.log\n");
    }
    let lock = String::from_utf8(read(repo.join("skiman.lock"))).unwrap();
    let snapshot = lock
        .lines()
        .find_map(|line| line.strip_prefix("snapshot = "))
        .unwrap();
    let new = format!("\"{}\"", tree_hash(&repo.join("upstream/pdf")));
    std::fs::write(repo.join("skiman.lock"), lock.replace(snapshot, &new)).unwrap();
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
    up
}

#[test]
fn a_changed_upstream_gitignore_from_an_earlier_install_stays_with_a_deletion_record() {
    let sandbox = Sandbox::new();
    installed_with_a_gitignore(&sandbox);
    let mine = b"*.log\n*.tmp\n";
    write(skill(&sandbox).join(".gitignore"), mine);

    let assert = sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(3)
        .stderr_eq("");

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert_eq!(
        conflict_list(&stdout),
        ["  .gitignore: deleted upstream; .gitignore.skiman-upstream-deleted marks it"]
    );
    let mut files = files(skill(&sandbox));
    files.remove("SKILL.md");
    assert_eq!(
        files,
        owned(&[
            (".gitignore", mine),
            (".gitignore.skiman-upstream-deleted", b"")
        ])
    );
    assert!(
        !sandbox
            .skill_repo()
            .join("upstream/pdf/.gitignore")
            .exists()
    );
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(1)
        .stdout_eq(
            "skills/pdf/.gitignore.skiman-upstream-deleted: unresolved-conflict: conflict record from an update; resolve the conflict, then delete it\n",
        );
}

const BIN_BASE: &[u8] = b"\0base binary";
const BIN_MINE: &[u8] = b"\0my binary";
const BIN_THEIRS: &[u8] = b"\0their binary";

#[test]
fn a_binary_file_changed_on_both_sides_keeps_mine_and_records_upstreams() {
    let (stdout, files) = conflicting_update(
        &[("logo.bin", BIN_BASE)],
        &[("logo.bin", Some(BIN_MINE))],
        &[("logo.bin", Some(BIN_THEIRS))],
    );
    assert_eq!(
        files,
        owned(&[
            ("logo.bin", BIN_MINE),
            ("logo.bin.skiman-upstream", BIN_THEIRS)
        ])
    );
    assert_eq!(
        conflict_list(&stdout),
        ["  logo.bin: upstream's version is in logo.bin.skiman-upstream"]
    );
}

#[test]
fn a_file_i_changed_and_upstream_deleted_stays_with_a_deletion_record() {
    let mine = lines(&[(3, "mine")]);
    let (stdout, files) = conflicting_update(
        &[("forms.md", lines(&[]).as_bytes())],
        &[("forms.md", Some(mine.as_bytes()))],
        &[("forms.md", None)],
    );
    assert_eq!(
        files,
        owned(&[
            ("forms.md", mine.as_bytes()),
            ("forms.md.skiman-upstream-deleted", b"")
        ])
    );
    assert_eq!(
        conflict_list(&stdout),
        ["  forms.md: deleted upstream; forms.md.skiman-upstream-deleted marks it"]
    );
}

#[test]
fn a_file_i_deleted_and_upstream_changed_comes_back_only_as_a_record() {
    let theirs = lines(&[(3, "theirs")]);
    let (stdout, files) = conflicting_update(
        &[("forms.md", lines(&[]).as_bytes())],
        &[("forms.md", None)],
        &[("forms.md", Some(theirs.as_bytes()))],
    );
    assert_eq!(
        files,
        owned(&[("forms.md.skiman-upstream", theirs.as_bytes())])
    );
    assert_eq!(
        conflict_list(&stdout),
        ["  forms.md: upstream's version is in forms.md.skiman-upstream"]
    );
}

#[test]
fn a_text_file_added_on_both_sides_gets_markers() {
    let (stdout, files) = conflicting_update(
        &[],
        &[("new.md", Some(b"mine\n"))],
        &[("new.md", Some(b"theirs\n"))],
    );
    assert_eq!(
        files,
        owned(&[(
            "new.md",
            b"<<<<<<< local (skiman)\nmine\n=======\ntheirs\n>>>>>>> upstream (skiman)\n"
        )])
    );
    assert_eq!(conflict_list(&stdout), ["  new.md: conflict markers"]);
}

#[test]
fn a_file_added_on_both_sides_with_one_side_binary_keeps_mine_and_records_upstreams() {
    for (mine, theirs) in [(BIN_MINE, BIN_THEIRS), (&b"text\n"[..], BIN_THEIRS)] {
        let (stdout, files) = conflicting_update(
            &[],
            &[("new.bin", Some(mine))],
            &[("new.bin", Some(theirs))],
        );
        assert_eq!(
            files,
            owned(&[("new.bin", mine), ("new.bin.skiman-upstream", theirs)])
        );
        assert_eq!(
            conflict_list(&stdout),
            ["  new.bin: upstream's version is in new.bin.skiman-upstream"]
        );
    }
}

#[test]
fn a_file_i_renamed_and_upstream_changed_conflicts_at_the_new_path() {
    let (stdout, files) = conflicting_update(
        &[("old.md", lines(&[]).as_bytes())],
        &[
            ("old.md", None),
            ("new.md", Some(lines(&[(5, "mine")]).as_bytes())),
        ],
        &[("old.md", Some(lines(&[(5, "theirs")]).as_bytes()))],
    );
    assert_eq!(
        files,
        BTreeMap::from([(
            "new.md".to_owned(),
            text_conflict("mine", "theirs").into_bytes()
        )])
    );
    assert_eq!(conflict_list(&stdout), ["  new.md: conflict markers"]);
}

#[test]
fn a_file_renamed_differently_on_both_sides_keeps_my_name_and_records_upstreams() {
    let text = lines(&[]);
    let (stdout, files) = conflicting_update(
        &[("a.md", text.as_bytes())],
        &[("a.md", None), ("mine.md", Some(text.as_bytes()))],
        &[("a.md", None), ("theirs.md", Some(text.as_bytes()))],
    );
    assert_eq!(
        files,
        owned(&[
            ("mine.md", text.as_bytes()),
            ("theirs.md.skiman-upstream", text.as_bytes())
        ])
    );
    assert_eq!(
        conflict_list(&stdout),
        ["  theirs.md: upstream's version is in theirs.md.skiman-upstream"]
    );
}

#[test]
fn upstream_files_in_a_folder_where_i_keep_a_file_go_under_skiman_conflicts() {
    let (stdout, files) = conflicting_update(
        &[],
        &[("docs", Some(b"my file\n"))],
        &[("docs/a.md", Some(b"a\n")), ("docs/b.md", Some(b"b\n"))],
    );
    assert_eq!(
        files,
        owned(&[
            (".skiman-conflicts/docs/a.md.skiman-upstream", b"a\n"),
            (".skiman-conflicts/docs/b.md.skiman-upstream", b"b\n"),
            ("docs", b"my file\n"),
        ])
    );
    assert_eq!(
        conflict_list(&stdout),
        [
            "  docs/a.md: upstream's version is in .skiman-conflicts/docs/a.md.skiman-upstream",
            "  docs/b.md: upstream's version is in .skiman-conflicts/docs/b.md.skiman-upstream",
        ]
    );
}

#[test]
fn an_upstream_file_where_i_keep_a_folder_is_recorded_next_to_it() {
    let (stdout, files) = conflicting_update(
        &[],
        &[("docs/a.md", Some(b"mine\n"))],
        &[("docs", Some(b"their file\n"))],
    );
    assert_eq!(
        files,
        owned(&[
            ("docs.skiman-upstream", b"their file\n"),
            ("docs/a.md", b"mine\n")
        ])
    );
    assert_eq!(
        conflict_list(&stdout),
        ["  docs: upstream's version is in docs.skiman-upstream"]
    );
}

#[test]
fn a_skill_with_unresolved_conflicts_is_refused_until_they_are_resolved() {
    let sandbox = Sandbox::new();
    let up = installed_with(
        &sandbox,
        &[
            ("reference.md", lines(&[]).as_bytes()),
            ("logo.bin", BIN_BASE),
        ],
    );
    let skill = skill(&sandbox);
    write(skill.join("reference.md"), lines(&[(5, "mine")]));
    write(skill.join("logo.bin"), BIN_MINE);
    up.write("reference.md", lines(&[(5, "theirs")]))
        .write("logo.bin", BIN_THEIRS);
    up.commit("v2");
    sandbox.skiman().args(["update", "pdf"]).assert().code(3);

    up.write("forms.md", "new upstream file\n");
    up.commit("v3");
    let before = skill_repo_tree(&sandbox);
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(
            "error: skill pdf: skills/pdf has unresolved conflicts; resolve them before updating:\n\
             \x20 skills/pdf/logo.bin.skiman-upstream: unresolved-conflict: [..]\n\
             \x20 skills/pdf/reference.md:5: unresolved-conflict: [..]\n\
             \x20 skills/pdf/reference.md:9: unresolved-conflict: [..]\n",
        );
    assert_eq!(skill_repo_tree(&sandbox), before);

    // Resolving the text conflict alone is not enough.
    write(skill.join("reference.md"), lines(&[(5, "mine and theirs")]));
    sandbox.skiman().args(["update", "pdf"]).assert().code(1);

    std::fs::remove_file(skill.join("logo.bin.skiman-upstream")).unwrap();
    let commit = up.head();
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {commit}; changed files:\n\
             \x20 added forms.md\n\
             saved the old folders and records of pdf to [..]\n"
        ));
    assert_eq!(
        read(skill.join("reference.md")),
        lines(&[(5, "mine and theirs")]).into_bytes()
    );
}

#[test]
fn a_hand_edited_snapshot_is_refused_with_the_check_line() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    write(
        sandbox.skill_repo().join("upstream/pdf/reference.md"),
        "edited by hand\n",
    );
    up.write("reference.md", lines(&[(5, "theirs")]));
    up.commit("v2");
    let before = skill_repo_tree(&sandbox);

    let check = sandbox.skiman().arg("check").assert().code(1);
    let check_line = String::from_utf8(check.get_output().stdout.clone())
        .unwrap()
        .lines()
        .find(|l| l.starts_with("upstream/pdf: snapshot-hash: "))
        .expect("check reports the snapshot")
        .to_owned();

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(format!(
            "error: skill pdf: upstream/pdf does not match skiman.lock; restore the snapshot before updating:\n  {check_line}\n"
        ));
    assert_eq!(skill_repo_tree(&sandbox), before);
}

fn skill_md(description: &str, body: &str) -> String {
    format!("---\nname: pdf\ndescription: {description}\n---\n\n{body}")
}

#[test]
fn markers_that_break_the_frontmatter_do_not_refuse_the_update() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    write(
        skill(&sandbox).join("SKILL.md"),
        skill_md("My description", "# pdf\n"),
    );
    up.write("SKILL.md", skill_md("Their description", "# pdf\n"));
    up.commit("v2");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(3)
        .stdout_eq(
            "skill pdf: updated to [..] with conflicts; changed files:\n\
             \x20 modified SKILL.md\n\
             saved the old folders and records of pdf to [..]\n\
             skill pdf: resolve these conflicts in skills/pdf, then delete the conflict records:\n\
             \x20 SKILL.md: conflict markers\n",
        )
        .stderr_eq("");
    assert_eq!(
        String::from_utf8(read(skill(&sandbox).join("SKILL.md"))).unwrap(),
        "---\nname: pdf\n<<<<<<< local (skiman)\ndescription: My description\n=======\n\
         description: Their description\n>>>>>>> upstream (skiman)\n---\n\n# pdf\n"
    );
}

#[test]
fn another_rule_violation_in_a_merged_file_refuses_a_conflicting_update() {
    // A clean upstream change breaks the frontmatter while the body conflicts.
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    write(
        skill(&sandbox).join("SKILL.md"),
        skill_md("Fill PDF forms", "# my pdf\n"),
    );
    up.write("SKILL.md", skill_md(&"x".repeat(1025), "# their pdf\n"));
    up.commit("v2");
    let before = skill_repo_tree(&sandbox);
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(
            "error: skill pdf: refusing to write content that breaks the rules:\n\
             \x20 skills/pdf/SKILL.md:3: description-length: [..]\n",
        );
    assert_eq!(skill_repo_tree(&sandbox), before);

    // A merged file without markers, next to a conflict in another file.
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    write(skill(&sandbox).join("reference.md"), lines(&[(5, "mine")]));
    up.write("reference.md", lines(&[(5, "theirs")]))
        .write("SKILL.md", "---\nname: pdf\n---\n");
    up.commit("v2");
    let before = skill_repo_tree(&sandbox);
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(
            "error: skill pdf: refusing to write content that breaks the rules:\n\
             \x20 skills/pdf/SKILL.md:[..]: required-field: [..]\n",
        );
    assert_eq!(skill_repo_tree(&sandbox), before);
}

#[test]
fn an_update_that_leaves_no_skill_md_is_refused() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    up.remove("SKILL.md");
    up.commit("v2");
    let before = skill_repo_tree(&sandbox);
    let links = || {
        let mut all = tree(sandbox.home());
        all.retain(|path, _| path.ends_with("skills/pdf"));
        all
    };
    let links_before = links();
    assert_eq!(links_before.len(), 3);
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(
            "error: skill pdf: refusing to write content that breaks the rules:\n\
             \x20 skills/pdf: skill-md: no SKILL.md at the top of the skill folder\n",
        );
    assert_eq!(skill_repo_tree(&sandbox), before);
    assert_eq!(links(), links_before);

    // A SKILL.md I changed and upstream deleted stays, with a deletion record.
    let mine = skill_md("Fill PDF forms", "# my pdf\n");
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    write(skill(&sandbox).join("SKILL.md"), &mine);
    up.remove("SKILL.md");
    up.commit("v2");
    sandbox.skiman().args(["update", "pdf"]).assert().code(3);
    assert_eq!(read(skill(&sandbox).join("SKILL.md")), mine.as_bytes());
    assert!(
        skill(&sandbox)
            .join("SKILL.md.skiman-upstream-deleted")
            .exists()
    );
}

#[test]
fn a_skill_md_i_deleted_and_upstream_changed_comes_back_only_as_a_record() {
    let theirs = skill_md("Fill PDF forms", "# their pdf\n");
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    std::fs::remove_file(skill(&sandbox).join("SKILL.md")).unwrap();
    up.write("SKILL.md", &theirs);
    let commit = up.commit("v2");
    let assert = sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(3)
        .stderr_eq(
            "warning: skill folder [..]/skills/pdf is not distributed: it holds no SKILL.md\n",
        );
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    // The links go too: a skill folder without SKILL.md is not distributed.
    assert_eq!(
        conflict_list(&stdout)[0],
        "  SKILL.md: upstream's version is in SKILL.md.skiman-upstream"
    );
    assert!(!skill(&sandbox).join("SKILL.md").exists());
    assert_eq!(
        read(skill(&sandbox).join("SKILL.md.skiman-upstream")),
        theirs.as_bytes()
    );
    let lock = String::from_utf8(read(sandbox.skill_repo().join("skiman.lock"))).unwrap();
    assert!(lock.contains(&format!("commit = \"{commit}\"")), "{lock}");
}

/// What the new upstream gains, and a word its refusal names.
type UpstreamChange<'a> = (&'a str, &'a dyn Fn(&Upstream));

#[test]
fn unsafe_upstream_content_appearing_on_update_is_refused() {
    let key = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
    let big = vec![b'x'; 10 * 1024 * 1024 + 1];
    let cases: [UpstreamChange; 4] = [
        ("symlink", &|up| {
            up.symlink("link.md", "reference.md");
        }),
        ("aws-access-key", &|up| {
            up.write("notes.md", format!("key {key}\n"));
        }),
        ("big.bin", &|up| {
            up.write("big.bin", &big);
        }),
        ("conflict-marker", &|up| {
            up.write("notes.md", "<<<<<<< local (skiman)\n");
        }),
    ];
    for (problem, change) in cases {
        let sandbox = Sandbox::new();
        let up = installed(&sandbox);
        write(skill(&sandbox).join("reference.md"), lines(&[(5, "mine")]));
        up.write("reference.md", lines(&[(5, "theirs")]));
        change(&up);
        up.commit("v2");
        let before = skill_repo_tree(&sandbox);

        let assert = sandbox
            .skiman()
            .args(["update", "pdf"])
            .assert()
            .code(1)
            .stdout_eq("");
        let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
        assert!(
            stderr.starts_with("error: skill pdf: ") && stderr.contains(problem),
            "{problem}: {stderr}"
        );
        assert!(!stderr.contains(&key));
        assert_eq!(skill_repo_tree(&sandbox), before, "{problem}");
    }
}

#[test]
fn a_case_collision_in_a_conflicting_merge_result_is_refused() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    write(skill(&sandbox).join("reference.md"), lines(&[(5, "mine")]));
    write(skill(&sandbox).join("Notes.md"), "mine\n");
    up.write("reference.md", lines(&[(5, "theirs")]))
        .write("notes.md", "theirs\n");
    up.commit("v2");
    let before = skill_repo_tree(&sandbox);

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq("error: skill pdf: the merge result holds [..]Notes.md[..]notes.md[..]\n");
    assert_eq!(skill_repo_tree(&sandbox), before);
}

#[test]
fn the_users_conflict_style_and_attributes_do_not_change_the_markers() {
    use std::io::Write;
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    write(skill(&sandbox).join("reference.md"), lines(&[(5, "mine")]));
    up.write("reference.md", lines(&[(5, "theirs")]));
    up.commit("v2");
    let attributes = sandbox.root().join("user-attributes");
    write(&attributes, "* conflict-marker-size=3 merge=union\n");
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(sandbox.git_config())
        .unwrap();
    writeln!(
        config,
        "[merge]\n\tconflictStyle = diff3\n[core]\n\tattributesFile = {}",
        attributes.display()
    )
    .unwrap();

    sandbox.skiman().args(["update", "pdf"]).assert().code(3);

    assert_eq!(
        String::from_utf8(read(skill(&sandbox).join("reference.md"))).unwrap(),
        text_conflict("mine", "theirs")
    );
}
