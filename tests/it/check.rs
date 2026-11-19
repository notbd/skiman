//! `skiman check`: every rule violation in the working tree, with path, line and rule.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use snapbox::str;

use crate::support::{Sandbox, hex, init, read, skill_md, tree_hash};

/// Writes `contents` at `rel` in the skill repo, creating folders, as a user editing by hand.
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

/// A skill repo holding `skills/<name>/SKILL.md` with `skill_md`.
fn repo_with_skill(name: &str, skill_md: &str) -> Sandbox {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(&sandbox, &format!("skills/{name}/SKILL.md"), skill_md);
    sandbox
}

/// A third-party skill built by hand: `skills/<name>` and `upstream/<name>`
/// with the same files, a `source` table and a lock entry whose snapshot is
/// the tree hash of the upstream snapshot.
fn third_party(sandbox: &Sandbox, name: &str) {
    for root in ["skills", "upstream"] {
        write(sandbox, &format!("{root}/{name}/SKILL.md"), skill_md(name));
        write(
            sandbox,
            &format!("{root}/{name}/scripts/run.sh"),
            "echo run\n",
        );
    }
    append(
        sandbox,
        "skiman.toml",
        &format!(
            "\n[skills.{name}.source]\nurl = \"https://github.com/acme/skills.git\"\npath = \"{name}\"\n"
        ),
    );
    let hash = tree_hash(&sandbox.skill_repo().join("upstream").join(name));
    let lock = sandbox.skill_repo().join("skiman.lock");
    if !lock.exists() {
        std::fs::write(&lock, "version = 1\n").unwrap();
    }
    append(
        sandbox,
        "skiman.lock",
        &format!(
            "\n[skills.{name}]\ncommit = \"{}\"\nsnapshot = \"{hash}\"\n",
            "a".repeat(40)
        ),
    );
}

fn check(sandbox: &Sandbox) -> snapbox::cmd::OutputAssert {
    sandbox.skiman().arg("check").assert()
}

#[test]
fn a_clean_skill_repo_passes_check_with_no_output() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "Fill PDF forms", "--group", "documents"])
        .assert()
        .code(0);
    third_party(&sandbox, "docx");
    sandbox
        .skiman()
        .args(["mv", "docx", "--group", "documents"])
        .assert()
        .code(0);
    append(
        &sandbox,
        "skiman.toml",
        "\n[skills.pdf]\nsummary = \"Fill PDF forms\"\norigin = \"written here\"\n\
         \n[skills.docx]\nsummary = \"Edit Word files\"\n",
    );

    check(&sandbox).code(0).stdout_eq("").stderr_eq("");
}

#[test]
fn a_skill_without_summary_group_or_origin_is_a_warning_of_check_only() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "Fill PDF forms"])
        .assert()
        .code(0)
        .stderr_eq("");
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes", "--group", "writing"])
        .assert()
        .code(0)
        .stderr_eq("");
    third_party(&sandbox, "docx");
    append(
        &sandbox,
        "skiman.toml",
        "\n[skills.notes]\nsummary = \"Keep notes\"\norigin = \"written here\"\n\
         \n[skills.docx]\n",
    );

    check(&sandbox).code(0).stdout_eq("").stderr_eq(str![[r#"
warning: skills/docx: skill-info: [skills.docx] in skiman.toml has no `summary`; it is in no group folder; move it into one with `skiman mv docx --group <group>`
warning: skills/pdf: skill-info: [skills.pdf] in skiman.toml has no `summary`, `origin`; it is in no group folder; move it into one with `skiman mv pdf --group <group>`

"#]]);

    // Neither the pre-commit hook's check nor the hooks' sync report it.
    let repo = sandbox.skill_repo();
    sandbox.git(&repo, ["add", "--all"]);
    sandbox
        .skiman()
        .args(["check", "--staged"])
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
}

#[test]
fn summary_origin_and_installed_must_have_their_type() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(&sandbox, "skills/pdf/SKILL.md", skill_md("pdf"));
    append(
        &sandbox,
        "skiman.toml",
        "\n[skills.pdf]\nsummary = \"Fill\\nforms\"\norigin = \" \"\ninstalled = \"2026-01-31\"\n",
    );

    check(&sandbox).code(1).stdout_eq(str![[r#"
skiman.toml:5: toml-value: `summary` in [skills.pdf] must be a non-empty string on one line
skiman.toml:6: toml-value: `origin` in [skills.pdf] must be a non-empty string on one line
skiman.toml:7: toml-value: `installed` in [skills.pdf] must be a date without a time, such as 2026-01-31

"#]]);

    let manifest = sandbox.skill_repo().join("skiman.toml");
    let text = read(&manifest).replace("installed = \"2026-01-31\"", "installed = 2026-01-31");
    std::fs::write(&manifest, text).unwrap();
    let out = check(&sandbox).code(1).get_output().stdout.clone();
    assert!(!String::from_utf8(out).unwrap().contains("installed"));
}

#[test]
fn a_skill_folder_without_skill_md_is_reported() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(&sandbox, "skills/pdf/README.md", "# pdf\n");
    write(&sandbox, "skills/notes.txt", "not a skill\n");

    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/notes.txt: skill-folder: not a folder; everything under skills/ must be a skill folder or a group folder of skill folders
skills/pdf: skill-md: no SKILL.md at the top of the skill folder

"#]]);
}

#[test]
fn frontmatter_that_does_not_start_on_line_1_is_reported() {
    let sandbox = repo_with_skill("pdf", "\n---\nname: pdf\ndescription: d\n---\n");
    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf/SKILL.md:1: frontmatter: the frontmatter must start on line 1 with `---`

"#]]);
}

#[test]
fn frontmatter_without_a_closing_line_is_reported() {
    let sandbox = repo_with_skill("pdf", "---\nname: pdf\ndescription: d\n# pdf\n");
    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf/SKILL.md:1: frontmatter: the frontmatter has no closing `---` line

"#]]);
}

#[test]
fn invalid_yaml_in_the_frontmatter_is_reported_with_its_line() {
    let sandbox = repo_with_skill("pdf", "---\nname: pdf\ndescription: [unclosed\n---\n");
    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf/SKILL.md:[..]: frontmatter: the frontmatter is not valid YAML: [..]

"#]]);
}

#[test]
fn a_duplicate_frontmatter_key_is_reported_on_the_line_of_the_second_key() {
    let sandbox = repo_with_skill("pdf", "---\nname: pdf\ndescription: d\nname: pdf\n---\n");
    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf/SKILL.md:4: duplicate-key: duplicate key in the frontmatter

"#]]);
}

#[test]
fn unknown_frontmatter_fields_are_accepted() {
    let sandbox = repo_with_skill(
        "pdf",
        "---\nname: pdf\ndescription: d\nlicense: MIT\nmetadata:\n  version: 2\nallowed-tools: [Read]\n---\n",
    );
    check(&sandbox).code(0).stdout_eq("");
}

#[test]
fn missing_name_and_description_are_reported() {
    let sandbox = repo_with_skill("pdf", "---\nlicense: MIT\n---\n");
    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf/SKILL.md:1: required-field: the frontmatter has no `description`
skills/pdf/SKILL.md:1: required-field: the frontmatter has no `name`

"#]]);
}

#[test]
fn a_name_that_breaks_the_name_rule_is_reported() {
    let sandbox = repo_with_skill("pdf", "---\ndescription: d\nname: PDF\n---\n");
    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf/SKILL.md:3: name-folder: `name` is `PDF` but the folder is `pdf`; they must be equal
skills/pdf/SKILL.md:3: name-format: `PDF` is not a valid skill name: use 1 to 64 lowercase letters, digits and single hyphens between them

"#]]);
}

#[test]
fn a_name_that_differs_from_the_folder_name_is_reported() {
    let sandbox = repo_with_skill("pdf", &skill_md("docx"));
    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf/SKILL.md:2: name-folder: `name` is `docx` but the folder is `pdf`; they must be equal

"#]]);
}

#[test]
fn a_reserved_name_in_the_frontmatter_is_reported() {
    let sandbox = repo_with_skill("pdf", &skill_md("synced"));
    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf/SKILL.md:2: name-folder: `name` is `synced` but the folder is `pdf`; they must be equal
skills/pdf/SKILL.md:2: name-reserved: `synced` is a reserved skill name

"#]]);
}

#[test]
fn skill_folders_with_reserved_invalid_or_too_long_names_are_reported() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let long = "a".repeat(65);
    for name in ["skiman-manual", ".hidden", "Bad_Name", long.as_str()] {
        write(&sandbox, &format!("skills/{name}/SKILL.md"), skill_md(name));
    }
    // Finder litter next to the skills is ignored.
    write(&sandbox, "skills/.DS_Store", "x");

    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/.hidden: skill-folder: `.hidden` is a reserved skill name (reserved: skiman-manual, synced, and any name starting with `.`)
skills/Bad_Name: skill-folder: `Bad_Name` is not a valid skill name: use 1 to 64 lowercase letters, digits and single hyphens between them
skills/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa: skill-folder: `aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa` is not a valid skill name: use 1 to 64 lowercase letters, digits and single hyphens between them
skills/skiman-manual: skill-folder: `skiman-manual` is a reserved skill name (reserved: skiman-manual, synced, and any name starting with `.`)

"#]]);
}

#[test]
fn a_description_of_0_or_more_than_1024_characters_is_reported() {
    let sandbox = repo_with_skill("pdf", "---\nname: pdf\ndescription: \"\"\n---\n");
    let long = "é".repeat(1025);
    write(
        &sandbox,
        "skills/docx/SKILL.md",
        format!("---\nname: docx\ndescription: {long}\n---\n"),
    );
    // Exactly 1024 characters (2048 bytes) passes: length counts characters.
    write(
        &sandbox,
        "skills/xlsx/SKILL.md",
        format!("---\nname: xlsx\ndescription: {}\n---\n", "é".repeat(1024)),
    );

    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/docx/SKILL.md:3: description-length: `description` must be 1 to 1024 characters (it has 1025)
skills/pdf/SKILL.md:3: description-length: `description` must be 1 to 1024 characters (it has 0)

"#]]);
}

#[test]
fn symlinks_inside_skills_are_reported() {
    let sandbox = repo_with_skill("pdf", &skill_md("pdf"));
    let repo = sandbox.skill_repo();
    std::os::unix::fs::symlink("SKILL.md", repo.join("skills/pdf/alias.md")).unwrap();
    std::fs::create_dir_all(repo.join("elsewhere")).unwrap();
    std::fs::write(repo.join("elsewhere/SKILL.md"), skill_md("docx")).unwrap();
    std::os::unix::fs::symlink(repo.join("elsewhere"), repo.join("skills/docx")).unwrap();

    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/docx: symlink: the skill folder is a symlink; skills must hold real files
skills/pdf/alias.md: symlink: symlinks are not allowed inside skills

"#]]);
}

/// A file of `size` bytes that takes no disk space.
fn sparse(path: &Path, size: u64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::File::create(path).unwrap().set_len(size).unwrap();
}

#[test]
fn a_file_over_10_mib_is_reported() {
    let sandbox = repo_with_skill("pdf", &skill_md("pdf"));
    let repo = sandbox.skill_repo();
    sparse(&repo.join("skills/pdf/assets/limit.bin"), 10 * 1024 * 1024);
    sparse(
        &repo.join("skills/pdf/assets/big.bin"),
        10 * 1024 * 1024 + 1,
    );

    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf/assets/big.bin: file-size: 10485761 bytes; a file in a skill may be at most 10 MiB

"#]]);
}

#[test]
fn more_than_1000_files_in_a_skill_are_reported() {
    let sandbox = repo_with_skill("pdf", &skill_md("pdf"));
    for i in 0..999 {
        write(&sandbox, &format!("skills/pdf/data/{i}.txt"), "x");
    }
    // `.DS_Store` files do not count.
    write(&sandbox, "skills/pdf/.DS_Store", "x");
    check(&sandbox).code(0).stdout_eq("");

    write(&sandbox, "skills/pdf/data/999.txt", "x");
    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf: file-count: 1001 files; a skill may hold at most 1000

"#]]);
}

#[test]
fn more_than_50_mib_in_a_skill_is_reported() {
    let sandbox = repo_with_skill("pdf", &skill_md("pdf"));
    let repo = sandbox.skill_repo();
    for i in 0..6 {
        sparse(
            &repo.join(format!("skills/pdf/assets/{i}.bin")),
            9 * 1024 * 1024,
        );
    }

    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf: skill-size: 56623[..] bytes in total; a skill may hold at most 50 MiB

"#]]);
}

#[test]
fn every_form_of_unresolved_conflict_is_reported() {
    let sandbox = repo_with_skill("pdf", &skill_md("pdf"));
    write(
        &sandbox,
        "skills/pdf/guide.md",
        "# Guide\n\n```text\n<<<<<<< local (skiman)\nours\n=======\ntheirs\n>>>>>>> upstream (skiman):guide.md\n```\n",
    );
    write(&sandbox, "skills/pdf/logo.png.skiman-upstream", "theirs");
    write(&sandbox, "skills/pdf/old.md.skiman-upstream-deleted", "");
    write(
        &sandbox,
        "skills/pdf/docs/.skiman-conflicts/a.md.skiman-upstream",
        "theirs",
    );
    // Conflict-like text that is not at the start of a line is fine.
    write(
        &sandbox,
        "skills/pdf/notes.md",
        "see <<<<<<< local (skiman)\n",
    );

    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/pdf/docs/.skiman-conflicts: unresolved-conflict: conflict records from an update; resolve them, then delete this folder
skills/pdf/docs/.skiman-conflicts/a.md.skiman-upstream: unresolved-conflict: conflict record from an update; resolve the conflict, then delete it
skills/pdf/guide.md:4: unresolved-conflict: conflict marker from an update; resolve the conflict and remove the markers
skills/pdf/guide.md:8: unresolved-conflict: conflict marker from an update; resolve the conflict and remove the markers
skills/pdf/logo.png.skiman-upstream: unresolved-conflict: conflict record from an update; resolve the conflict, then delete it
skills/pdf/old.md.skiman-upstream-deleted: unresolved-conflict: conflict record from an update; resolve the conflict, then delete it

"#]]);
}

#[test]
fn unknown_keys_in_skiman_toml_are_reported_with_their_line() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    third_party(&sandbox, "pdf");
    append(
        &sandbox,
        "skiman.toml",
        "branch = \"main\"\n\n[skills.pdf]\nenabled = \"yes\"\npinned = true\n\n[colors]\nred = 1\n",
    );

    check(&sandbox).code(1).stdout_eq(str![[r#"
skiman.toml:7: unknown-key: unknown key `branch` in [skills.pdf.source] (allowed: url, path, ref, ref_kind, upstream_name)
skiman.toml:10: toml-value: `enabled` in [skills.pdf] must be true or false
skiman.toml:11: unknown-key: unknown key `pinned` in [skills.pdf] (allowed: enabled, summary, origin, installed, source)
skiman.toml:13: unknown-key: unknown key `colors` in skiman.toml (allowed: tools, skills, secrets, projects, linked)

"#]]);
}

#[test]
fn a_source_table_without_url_or_with_a_bad_ref_kind_is_reported() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    third_party(&sandbox, "pdf");
    let manifest = sandbox.skill_repo().join("skiman.toml");
    let text = std::fs::read_to_string(&manifest).unwrap().replace(
        "url = \"https://github.com/acme/skills.git\"\n",
        "ref_kind = \"sha\"\n",
    );
    std::fs::write(&manifest, text).unwrap();

    check(&sandbox).code(1).stdout_eq(str![[r#"
skiman.toml:4: toml-value: [skills.pdf.source] has no `url` (a string)
skiman.toml:5: toml-value: `ref_kind` in [skills.pdf.source] must be one of branch, tag, commit

"#]]);
}

#[test]
fn invalid_toml_is_reported_with_its_line() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    append(&sandbox, "skiman.toml", "\n[skills.pdf\n");

    check(&sandbox).code(1).stdout_eq(str![[r#"
skiman.toml:4: toml-syntax: not valid TOML: [..]

"#]]);
}

#[test]
fn a_lockfile_that_breaks_the_version_1_format_is_reported() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    third_party(&sandbox, "pdf");
    let lock = sandbox.skill_repo().join("skiman.lock");
    let text = std::fs::read_to_string(&lock)
        .unwrap()
        .replace("version = 1", "version = 2")
        .replace("commit = \"", "date = \"today\"\ncommit = \"x");
    std::fs::write(&lock, text).unwrap();

    check(&sandbox).code(1).stdout_eq(str![[r#"
skiman.lock:1: lock-version: skiman.lock must have `version = 1`
skiman.lock:4: unknown-key: unknown key `date` in lock entry [skills.pdf] (allowed: commit, ref, snapshot)
skiman.lock:5: toml-value: `commit` in lock entry [skills.pdf] must be a 40-hex commit sha

"#]]);
}

#[test]
fn the_tree_hash_follows_the_spec_ignoring_finder_files_and_empty_folders() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    third_party(&sandbox, "pdf");
    let upstream = sandbox.skill_repo().join("upstream/pdf");

    // Worked example: the hashed text written out by hand.
    let skill_md = skill_md("pdf");
    let by_hand = format!(
        "SKILL.md\x000\x00{}\nscripts/run.sh\x000\x00{}\n",
        hex(&Sha256::digest(skill_md.as_bytes())),
        hex(&Sha256::digest(b"echo run\n")),
    );
    let by_hand = format!("sha256:{}", hex(&Sha256::digest(by_hand.as_bytes())));
    assert_eq!(tree_hash(&upstream), by_hand);
    check(&sandbox).code(0).stdout_eq("");

    std::fs::write(upstream.join(".DS_Store"), "finder").unwrap();
    std::fs::write(upstream.join("scripts/Thumbs.db"), "windows").unwrap();
    std::fs::create_dir_all(upstream.join("empty/nested")).unwrap();
    check(&sandbox).code(0).stdout_eq("");

    let script = upstream.join("scripts/run.sh");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let flipped = tree_hash(&upstream);
    assert_ne!(flipped, by_hand);
    check(&sandbox).code(1).stdout_eq(format!(
        "upstream/pdf: snapshot-hash: tree hash {flipped} does not match the snapshot {by_hand} in skiman.lock; upstream snapshots are never edited by hand, so restore it (if an update stopped part-way, run `skiman update pdf` again)\n"
    ));
}

#[test]
fn a_hand_edited_upstream_snapshot_is_a_hash_mismatch() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    third_party(&sandbox, "pdf");
    write(&sandbox, "upstream/pdf/scripts/run.sh", "echo edited\n");
    // Upstream snapshots are exempt from the skill-format rules.
    write(&sandbox, "upstream/pdf/SKILL.md", "no frontmatter\n");

    check(&sandbox).code(1).stdout_eq(str![[r#"
upstream/pdf: snapshot-hash: tree hash sha256:[..] does not match the snapshot sha256:[..] in skiman.lock; upstream snapshots are never edited by hand, so restore it (if an update stopped part-way, run `skiman update pdf` again)

"#]]);
}

#[test]
fn renaming_a_third_party_skill_everywhere_by_hand_passes() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    third_party(&sandbox, "pdf");
    let repo = sandbox.skill_repo();
    std::fs::rename(repo.join("skills/pdf"), repo.join("skills/forms")).unwrap();
    std::fs::rename(repo.join("upstream/pdf"), repo.join("upstream/forms")).unwrap();
    write(&sandbox, "skills/forms/SKILL.md", skill_md("forms"));
    for file in ["skiman.toml", "skiman.lock"] {
        let text = std::fs::read_to_string(repo.join(file))
            .unwrap()
            .replace("[skills.pdf", "[skills.forms");
        std::fs::write(repo.join(file), text).unwrap();
    }

    check(&sandbox).code(0).stdout_eq("");
}

#[test]
fn an_incomplete_hand_rename_is_reported_piece_by_piece() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    third_party(&sandbox, "pdf");
    let repo = sandbox.skill_repo();
    std::fs::rename(repo.join("skills/pdf"), repo.join("skills/forms")).unwrap();
    write(&sandbox, "skills/forms/SKILL.md", skill_md("forms"));
    let text = std::fs::read_to_string(repo.join("skiman.toml"))
        .unwrap()
        .replace("[skills.pdf", "[skills.forms");
    std::fs::write(repo.join("skiman.toml"), text).unwrap();

    check(&sandbox).code(3).stdout_eq(str![[r#"
skiman.lock:3: leftover-record: leftover lock entry for pdf, but skills/pdf does not exist; run `skiman remove pdf --yes` (after a hand rename, rename this too)
skiman.toml:4: consistency: [skills.forms.source] has no entry in skiman.lock
skiman.toml:4: consistency: [skills.forms.source] has no upstream snapshot upstream/forms
upstream/pdf: leftover-record: leftover upstream snapshot, but skills/pdf does not exist; run `skiman remove pdf --yes` (after a hand rename, rename this too)

"#]]);
}

#[test]
fn converting_to_own_names_each_piece_still_left() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    third_party(&sandbox, "pdf");
    let repo = sandbox.skill_repo();
    let manifest_before = std::fs::read_to_string(repo.join("skiman.toml")).unwrap();
    let own_manifest = manifest_before
        .split("\n[skills.pdf.source]")
        .next()
        .unwrap()
        .to_owned();

    // Source table deleted: the lock entry and the snapshot are left.
    std::fs::write(repo.join("skiman.toml"), &own_manifest).unwrap();
    check(&sandbox).code(1).stdout_eq(str![[r#"
skiman.lock:3: consistency: lock entry for pdf, but skills/pdf has no [skills.pdf.source] table, so it is an own skill; delete the entry, or restore the source table
upstream/pdf: consistency: upstream snapshot, but skills/pdf has no [skills.pdf.source] table, so it is an own skill; delete the snapshot, or restore the source table

"#]]);

    // Source table and lock entry deleted: the snapshot is left.
    std::fs::write(repo.join("skiman.lock"), "version = 1\n").unwrap();
    check(&sandbox).code(1).stdout_eq(str![[r#"
upstream/pdf: consistency: upstream snapshot, but skills/pdf has no [skills.pdf.source] table, so it is an own skill; delete the snapshot, or restore the source table

"#]]);

    // Lock entry and snapshot deleted, source table kept: the source table is named.
    std::fs::write(repo.join("skiman.toml"), &manifest_before).unwrap();
    std::fs::remove_dir_all(repo.join("upstream/pdf")).unwrap();
    check(&sandbox).code(1).stdout_eq(str![[r#"
skiman.toml:4: consistency: [skills.pdf.source] has no entry in skiman.lock
skiman.toml:4: consistency: [skills.pdf.source] has no upstream snapshot upstream/pdf

"#]]);

    // All three deleted: a clean own skill.
    std::fs::write(repo.join("skiman.toml"), &own_manifest).unwrap();
    check(&sandbox).code(0).stdout_eq("");
}

#[test]
fn a_hand_deleted_third_party_skill_folder_leaves_records_that_need_attention() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    third_party(&sandbox, "pdf");
    std::fs::remove_dir_all(sandbox.skill_repo().join("skills/pdf")).unwrap();

    check(&sandbox).code(3).stdout_eq(str![[r#"
skiman.lock:3: leftover-record: leftover lock entry for pdf, but skills/pdf does not exist; run `skiman remove pdf --yes` (after a hand rename, rename this too)
skiman.toml:4: leftover-record: leftover [skills.pdf.source] table, but skills/pdf does not exist; run `skiman remove pdf --yes` (after a hand rename, rename this too)
upstream/pdf: leftover-record: leftover upstream snapshot, but skills/pdf does not exist; run `skiman remove pdf --yes` (after a hand rename, rename this too)

"#]]);
}

#[test]
fn leftover_records_and_rule_violations_together_exit_3() {
    let sandbox = repo_with_skill("docx", &skill_md("pdf"));
    third_party(&sandbox, "pdf");
    std::fs::remove_dir_all(sandbox.skill_repo().join("skills/pdf")).unwrap();

    check(&sandbox).code(3);
}

#[test]
fn check_without_a_user_config_is_an_operational_error() {
    let sandbox = Sandbox::new();
    check(&sandbox).code(4).stdout_eq("").stderr_eq(str![[r#"
error: [..]/config.toml does not exist; run `skiman init <path>` first

"#]]);
}

#[test]
fn new_succeeds_while_another_skill_breaks_a_rule_and_warns_about_it() {
    let sandbox = repo_with_skill("docx", "no frontmatter\n");

    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "Fill PDF forms"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
created [..]/skills/pdf
...
"#]])
        .stderr_eq(str![[r#"
warning: skills/docx/SKILL.md:1: frontmatter: the frontmatter must start on line 1 with `---`

"#]]);
    assert!(sandbox.skill_repo().join("skills/pdf/SKILL.md").is_file());
}

#[test]
fn a_skiman_toml_that_does_not_parse_gives_no_skill_info_warning() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(&sandbox, "skills/pdf/SKILL.md", skill_md("pdf"));
    append(
        &sandbox,
        "skiman.toml",
        "\n[skills.pdf\nsummary = \"Fill PDF forms\"\n",
    );

    let output = check(&sandbox).code(1);
    let stderr = String::from_utf8(output.get_output().stderr.clone()).unwrap();
    assert!(!stderr.contains("skill-info"), "{stderr}");
}

/// `skills/big/SKILL.md` whose body, without its surrounding blank lines, is `bytes` long.
fn write_skill_body(sandbox: &Sandbox, bytes: usize) {
    write(
        sandbox,
        "skills/big/SKILL.md",
        format!(
            "---\nname: big\ndescription: d\n---\n\n\n{}\n\n",
            "x".repeat(bytes)
        ),
    );
}

/// The `skill-body-size` lines `check` prints; asserts exit 0.
fn body_size_warnings(sandbox: &Sandbox) -> Vec<String> {
    let output = check(sandbox).code(0);
    String::from_utf8(output.get_output().stderr.clone())
        .unwrap()
        .lines()
        .filter(|line| line.contains("skill-body-size"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_skill_body_over_antigravitys_limit_is_a_warning_while_antigravity_is_enabled() {
    let sandbox = Sandbox::new();
    init(&sandbox);

    // Measured with agy 1.5.12: it keeps 24000 bytes of the body, counted
    // without the frontmatter and without surrounding whitespace.
    write_skill_body(&sandbox, 23999);
    assert_eq!(body_size_warnings(&sandbox), Vec::<String>::new());
    write_skill_body(&sandbox, 24000);
    assert_eq!(body_size_warnings(&sandbox), Vec::<String>::new());

    write_skill_body(&sandbox, 24001);
    assert_eq!(
        body_size_warnings(&sandbox),
        [
            "warning: skills/big/SKILL.md: skill-body-size: the body is 24001 bytes (without the frontmatter and surrounding whitespace); antigravity cuts it after 24000 bytes without a message"
        ]
    );

    write(
        &sandbox,
        "skiman.toml",
        "[tools]\nenabled = [\"claude-code\", \"codex\"]\n",
    );
    assert_eq!(body_size_warnings(&sandbox), Vec::<String>::new());
}
