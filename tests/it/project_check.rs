//! `skiman check` and `skiman doctor` on the project scope (project-scope.md
//! "Rules" and "Other changes"): `project-path` on the layout of `projects/`,
//! the gate 2 safety rules on every mirror item, and doctor's project section.
//!
//! Secret-like fixtures are built at runtime from pieces, so no literal token
//! is committed (GitHub push protection would block it).

use std::path::{Path, PathBuf};

use crate::support::{Sandbox, aws_key, fingerprint, init, read, write};

fn append(path: impl AsRef<Path>, text: &str) {
    let mut old = read(&path);
    old.push_str(text);
    std::fs::write(path, old).unwrap();
}

/// A canon with the project root `~/work` registered as `work`.
fn with_root() -> (Sandbox, PathBuf) {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let root = sandbox.home().join("work");
    std::fs::create_dir_all(&root).unwrap();
    sandbox
        .skiman()
        .args(["project", "add", "--id", "work"])
        .arg(&root)
        .assert()
        .code(0);
    (sandbox, root)
}

/// `projects/<rel>` in the canon.
fn mirror(sandbox: &Sandbox, rel: &str) -> PathBuf {
    sandbox.canon().join("projects").join(rel)
}

fn adopt(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
}

/// Exit code and stdout of `skiman check`.
fn check(sandbox: &Sandbox) -> (i32, String) {
    let output = sandbox.skiman().arg("check").output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
    )
}

/// Stdout of `skiman doctor`, which exits 0 whenever the diagnosis ran.
fn doctor(sandbox: &Sandbox) -> String {
    let output = sandbox.skiman().arg("doctor").output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn assert_has(text: &str, line: &str) {
    assert!(
        text.lines().any(|l| l == line),
        "missing line:\n{line}\nin:\n{text}"
    );
}

fn assert_lacks(text: &str, part: &str) {
    assert!(!text.contains(part), "unexpected `{part}` in:\n{text}");
}

fn git_commit_all(sandbox: &Sandbox, dir: &Path) {
    sandbox.git(dir, ["add", "-A"]);
    sandbox.git(
        dir,
        [
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "mirror",
        ],
    );
}

const GIT_CONFLICT: &str = "intro\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> other\n";

// ---------------------------------------------------------------- check: layout

#[test]
fn a_file_under_a_root_that_belongs_to_no_item_is_a_project_path_violation() {
    let (sandbox, _) = with_root();
    write(mirror(&sandbox, "work/proj/AGENTS.md"), "rules\n");
    write(mirror(&sandbox, "work/proj/notes.txt"), "scratch\n");

    let (code, stdout) = check(&sandbox);
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        "projects/work/proj/notes.txt: project-path: belongs to no project item (an instruction file or a project skill); restore the configuration that made it one, or `git rm -r projects/work/proj/notes.txt`\n"
    );
}

#[test]
fn a_mirror_folder_of_an_unknown_root_id_is_a_project_path_violation() {
    let (sandbox, _) = with_root();
    write(mirror(&sandbox, "gone/proj/AGENTS.md"), "rules\n");

    let (code, stdout) = check(&sandbox);
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        "projects/gone: project-path: `gone` is not a configured project root; add [projects.gone] to skiman.toml again, or `git rm -r projects/gone`\n"
    );
}

/// What a removed `skill_dirs` key leaves in the mirror (ADR 0013): a
/// skill folder outside every tool's project skill folder has no item
/// shape, and its files are not told to restore a configuration that is
/// gone.
#[test]
fn a_former_skill_dirs_mirror_item_is_a_path_without_an_item_shape() {
    let (sandbox, _) = with_root();
    write(
        mirror(&sandbox, "work/lib/skills/tool/SKILL.md"),
        "# tool\n",
    );
    write(
        mirror(&sandbox, "work/lib/skills/tool/scripts/run.sh"),
        "echo\n",
    );

    let (code, stdout) = check(&sandbox);
    assert_eq!(code, 1);
    let line = |path: &str| {
        format!(
            "{path}: project-path: belongs to no project item; it is left from the retired `skill_dirs` (a skill folder outside every project skill folder), so `git rm -r {path}`, and register the original folder with `skiman linked add` if it is still in use\n"
        )
    };
    assert_eq!(
        stdout,
        line("projects/work/lib/skills/tool/SKILL.md")
            + &line("projects/work/lib/skills/tool/scripts/run.sh")
    );
}

// ---------------------------------------------------------------- check: item rules

#[test]
fn a_secret_in_a_mirror_file_is_reported_and_an_allow_entry_for_its_projects_path_exempts_it() {
    let (sandbox, _) = with_root();
    let key = aws_key();
    write(
        mirror(&sandbox, "work/proj/AGENTS.md"),
        format!("rules\nAWS_ACCESS_KEY_ID={key}\n"),
    );
    let fp = fingerprint(&key);

    let (code, stdout) = check(&sandbox);
    assert_eq!(code, 1);
    assert_has(
        &stdout,
        &format!(
            "projects/work/proj/AGENTS.md:2: aws-access-key: looks like a secret; fingerprint = \"{fp}\""
        ),
    );
    assert_lacks(&stdout, &key);

    append(
        sandbox.canon().join("skiman.toml"),
        &format!(
            "\n[[secrets.allow]]\npath = \"projects/work/proj/AGENTS.md\"\nrule = \"aws-access-key\"\nfingerprint = \"{fp}\"\n"
        ),
    );
    assert_eq!(check(&sandbox), (0, String::new()));
}

#[test]
fn a_secret_in_a_file_of_a_mirror_skill_is_reported() {
    let (sandbox, _) = with_root();
    let key = aws_key();
    let skill = "work/proj/.claude/skills/deploy";
    write(mirror(&sandbox, &format!("{skill}/SKILL.md")), "# deploy\n");
    write(
        mirror(&sandbox, &format!("{skill}/env.sh")),
        format!("export AWS_ACCESS_KEY_ID={key}\n"),
    );

    let (code, stdout) = check(&sandbox);
    assert_eq!(code, 1);
    assert_has(
        &stdout,
        &format!(
            "projects/{skill}/env.sh:1: aws-access-key: looks like a secret; fingerprint = \"{}\"",
            fingerprint(&key)
        ),
    );
}

#[test]
fn a_secret_in_a_stray_file_under_projects_is_reported_too() {
    let (sandbox, _) = with_root();
    let key = aws_key();
    write(
        mirror(&sandbox, "work/proj/notes.env"),
        format!("AWS_ACCESS_KEY_ID={key}\n"),
    );

    let (code, stdout) = check(&sandbox);
    assert_eq!(code, 1);
    assert_has(
        &stdout,
        &format!(
            "projects/work/proj/notes.env:1: aws-access-key: looks like a secret; fingerprint = \"{}\"",
            fingerprint(&key)
        ),
    );
}

#[test]
fn git_conflict_markers_in_a_mirror_instruction_file_are_unresolved_conflicts() {
    let (sandbox, _) = with_root();
    write(mirror(&sandbox, "work/proj/AGENTS.md"), GIT_CONFLICT);

    let (code, stdout) = check(&sandbox);
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        "projects/work/proj/AGENTS.md:2: unresolved-conflict: conflict marker; resolve the conflict and remove the markers\n\
         projects/work/proj/AGENTS.md:6: unresolved-conflict: conflict marker; resolve the conflict and remove the markers\n"
    );
}

#[test]
fn git_conflict_markers_in_a_mirror_skill_file_are_unresolved_conflicts() {
    let (sandbox, _) = with_root();
    let skill = "work/proj/.claude/skills/deploy";
    write(mirror(&sandbox, &format!("{skill}/SKILL.md")), "# deploy\n");
    write(mirror(&sandbox, &format!("{skill}/steps.md")), GIT_CONFLICT);

    let (code, stdout) = check(&sandbox);
    assert_eq!(code, 1);
    let reported: Vec<&str> = stdout
        .lines()
        .filter(|l| l.contains(": unresolved-conflict: "))
        .collect();
    assert_eq!(
        reported.len(),
        2,
        "one violation per marker line:\n{stdout}"
    );
    assert!(reported[0].starts_with(&format!("projects/{skill}/steps.md:2: ")));
    assert!(reported[1].starts_with(&format!("projects/{skill}/steps.md:6: ")));
}

#[test]
fn a_mirror_skill_without_skill_md_is_reported() {
    let (sandbox, _) = with_root();
    write(
        mirror(&sandbox, "work/proj/.claude/skills/deploy/steps.md"),
        "steps\n",
    );

    let (code, stdout) = check(&sandbox);
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        "projects/work/proj/.claude/skills/deploy: skill-md: no SKILL.md at the top of the skill folder\n"
    );
}

#[test]
fn a_mirror_skill_without_frontmatter_name_in_a_folder_of_any_name_passes() {
    let (sandbox, _) = with_root();
    write(
        mirror(&sandbox, "work/proj/.claude/skills/Weekly_PO/SKILL.md"),
        "---\ndescription: Weekly purchase orders\n---\n\n# Weekly PO\n",
    );
    write(
        mirror(&sandbox, "work/proj/.agents/skills/no-frontmatter/SKILL.md"),
        "# Just a body\n",
    );

    assert_eq!(check(&sandbox), (0, String::new()));
}

// ---------------------------------------------------------------- doctor

#[test]
fn doctor_prints_each_root_with_its_mirror_item_count() {
    let (sandbox, root) = with_root();
    write(root.join("a/AGENTS.md"), "a\n");
    write(root.join("b/.claude/skills/x/SKILL.md"), "# x\n");
    adopt(&sandbox);

    assert_has(
        &doctor(&sandbox),
        &format!(
            "project root work: {} (2 item(s) in the mirror)",
            root.display()
        ),
    );
}

#[test]
fn doctor_lists_every_item_not_in_step_with_its_fix_and_changes_nothing() {
    let (sandbox, root) = with_root();
    for project in ["a", "b", "c"] {
        write(root.join(project).join("AGENTS.md"), "rules\n");
    }
    adopt(&sandbox);
    write(
        mirror(&sandbox, "work/a/AGENTS.md"),
        "edited in the mirror\n",
    );
    std::fs::remove_file(root.join("b/AGENTS.md")).unwrap();

    let stdout = doctor(&sandbox);
    assert_has(
        &stdout,
        &format!(
            "{a}: its mirror changed; write it to the project: skiman project restore {a}; or keep the project's version: skiman project adopt {a}",
            a = root.join("a/AGENTS.md").display()
        ),
    );
    assert_has(
        &stdout,
        &format!(
            "{b}: it was deleted in the project; bring it back from the mirror: skiman project restore {b}; or stop managing it: skiman project eject {b}",
            b = root.join("b/AGENTS.md").display()
        ),
    );
    assert_lacks(&stdout, &root.join("c/AGENTS.md").display().to_string());
    assert_eq!(read(root.join("a/AGENTS.md")), "rules\n");
    assert!(!root.join("b/AGENTS.md").exists());
}

#[test]
fn doctor_counts_candidates_that_are_not_adopted_yet() {
    let (sandbox, root) = with_root();
    write(root.join("a/AGENTS.md"), "a\n");
    adopt(&sandbox);
    assert_lacks(&doctor(&sandbox), "not adopted yet");

    write(root.join("b/AGENTS.md"), "b\n");
    write(root.join("b/.claude/rules/style.md"), "style\n");
    assert_has(
        &doctor(&sandbox),
        &format!(
            "project root work: 2 project item(s) are not adopted yet; see them with: skiman project adopt {} --dry-run",
            root.display()
        ),
    );
}

#[test]
fn doctor_lists_a_swap_folder_left_in_a_project_and_does_not_count_what_it_holds() {
    let (sandbox, root) = with_root();
    write(root.join("proj/AGENTS.md"), "rules\n");
    adopt(&sandbox);
    let leftover = root.join("proj/.skiman-swap-20260101T000000Z");
    write(leftover.join("old/AGENTS.md"), "older rules\n");
    write(leftover.join("target"), "proj/AGENTS.md");

    let stdout = doctor(&sandbox);
    assert_has(
        &stdout,
        &format!(
            "{} is left from an earlier `skiman project restore`; the next restore there moves it to a backup",
            leftover.display()
        ),
    );
    assert_lacks(&stdout, "not adopted yet");
    assert_eq!(read(leftover.join("old/AGENTS.md")), "older rules\n");
}

#[test]
fn doctor_reports_uncommitted_mirror_changes_until_they_are_committed() {
    let (sandbox, root) = with_root();
    write(root.join("a/AGENTS.md"), "a\n");
    write(root.join("b/AGENTS.md"), "b\n");
    adopt(&sandbox);
    assert_has(
        &doctor(&sandbox),
        "projects: 2 uncommitted change(s); skiman never commits, so commit them to keep their history",
    );

    git_commit_all(&sandbox, &sandbox.canon());
    assert_lacks(&doctor(&sandbox), "uncommitted change");
}

#[test]
fn doctor_reports_a_managed_item_that_a_project_git_repo_tracks_later() {
    let (sandbox, root) = with_root();
    let project = root.join("proj");
    let item = project.join("AGENTS.md");
    write(&item, "rules\n");
    sandbox.git(&project, ["init", "-q"]);
    adopt(&sandbox);
    assert_lacks(&doctor(&sandbox), "now tracked");

    sandbox.git(&project, ["add", "AGENTS.md"]);
    assert_has(
        &doctor(&sandbox),
        &format!(
            "{}: now tracked by the git repo {}; leave it to that repo with: skiman project eject {}",
            item.display(),
            project.display(),
            item.display()
        ),
    );
}

#[test]
fn doctor_reports_a_project_change_the_rules_block() {
    let (sandbox, root) = with_root();
    write(root.join("a/AGENTS.md"), "a\n");
    adopt(&sandbox);
    write(
        root.join("a/AGENTS.md"),
        format!("key = AKIA{}\n", "ABCDEFGHIJKLMNOP"),
    );

    let stdout = doctor(&sandbox);
    let line = format!(
        "{}: it is not copied into the mirror: projects/work/a/AGENTS.md:1: aws-access-key",
        root.join("a/AGENTS.md").display()
    );
    assert!(stdout.lines().any(|l| l.starts_with(&line)), "{stdout}");
}

#[test]
fn a_mirror_folder_left_with_only_ignored_files_is_not_a_violation() {
    let (sandbox, _root) = with_root();
    write(sandbox.canon().join("projects/gone/.DS_Store"), "x");
    sandbox.skiman().arg("check").assert().code(0);
}
