//! `skiman add` with every source form (GitHub tree and blob URLs, other git
//! URLs, local folders), `--ref`, and skill discovery and selection in an
//! upstream that holds several skills.

use std::path::{Path, PathBuf};

use snapbox::str;

use crate::support::{Sandbox, Tool, Upstream, tree};

fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

/// The whole skill repo and every tool directory, to prove a command changed nothing.
fn world(sandbox: &Sandbox) -> Vec<std::collections::BTreeMap<PathBuf, String>> {
    let mut world = vec![tree(sandbox.skill_repo())];
    world.extend(Tool::ALL.iter().map(|t| tree(sandbox.tool_dir(*t))));
    world
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: The {name} skill\n---\n\n# {name}\n")
}

/// `acme/skills` with skills `pdf` and `docx` side by side under `skills/`.
fn flat_repo(sandbox: &Sandbox) -> Upstream {
    let up = sandbox.upstream("acme/skills");
    up.write("README.md", "# Skills\n")
        .write("skills/pdf/SKILL.md", skill_md("pdf"))
        .write("skills/pdf/reference.md", "# pdf reference\n")
        .write("skills/docx/SKILL.md", skill_md("docx"));
    up.commit("v1");
    up
}

#[test]
fn refused_source_forms_exit_1_and_write_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let before = world(&sandbox);
    let unsupported = "an https:// or ssh:// git URL, user@host:path, a local folder starting with `/`, `~` or `.`, a GitHub tree or blob URL, or <owner>/<repo>";
    for (source, message) in [
        (
            "-uhelper".to_owned(),
            "the source `-uhelper` starts with `-`; sources never do".to_owned(),
        ),
        (
            "ext::sh -c touch% /tmp/pwned".to_owned(),
            "the source `ext::sh -c touch% /tmp/pwned` uses a git remote helper (`::`), which skiman never runs".to_owned(),
        ),
        (
            "https://example.test/x::y".to_owned(),
            "the source `https://example.test/x::y` uses a git remote helper (`::`), which skiman never runs".to_owned(),
        ),
        (
            "file:///tmp/repo".to_owned(),
            format!("unsupported source `file:///tmp/repo`: give {unsupported}"),
        ),
        (
            "git://example.test/repo.git".to_owned(),
            format!("unsupported source `git://example.test/repo.git`: give {unsupported}"),
        ),
        (
            "http://example.test/repo.git".to_owned(),
            format!("unsupported source `http://example.test/repo.git`: give {unsupported}"),
        ),
        (
            "pdf".to_owned(),
            format!("unsupported source `pdf`: give {unsupported}"),
        ),
        (
            "https://github.com/acme/skills/blob/main/skills/pdf/README.md".to_owned(),
            "the blob URL `https://github.com/acme/skills/blob/main/skills/pdf/README.md` does not point at a SKILL.md; give the URL of the skill's SKILL.md or of its folder".to_owned(),
        ),
    ] {
        sandbox
            .skiman()
            .args(["add", "--yes", "--", &source])
            .assert()
            .code(1)
            .stdout_eq("")
            .stderr_eq(format!("error: {message}\n"));
    }
    assert_eq!(world(&sandbox), before);
}

#[test]
fn dry_run_on_a_multi_skill_source_without_a_selection_lists_the_candidates() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    flat_repo(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/skills", "--dry-run"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
https://github.com/acme/skills.git at [..] (default branch main) holds 2 skills; choose with --skill <name> or --all:
  docx  (skills/docx)
  pdf  (skills/pdf)
dry run: nothing was written

"#]])
        .stderr_eq("");

    assert_eq!(world(&sandbox), before);
}

/// The candidate lines of `add <source> --dry-run` without a selection.
fn candidates(sandbox: &Sandbox, source: &str) -> String {
    let output = sandbox
        .skiman()
        .args(["add", source, "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("  "))
        .map(|l| format!("{}\n", l.trim()))
        .collect()
}

#[test]
fn a_root_with_skill_md_is_the_only_candidate() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", skill_md("pdf"))
        .write("examples/demo/SKILL.md", skill_md("demo"));
    up.commit("v1");

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--dry-run"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skill pdf from https://github.com/acme/pdf.git at [..] (default branch main):
  SKILL.md
  examples/demo/SKILL.md
dry run: nothing was written

"#]]);
}

#[test]
fn grouped_skills_in_category_folders_are_found_at_every_depth() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = sandbox.upstream("acme/grouped");
    up.write("docs/SKILL.md.txt", "not a skill\n")
        .write("office/pdf/SKILL.md", skill_md("pdf"))
        .write("office/docx/SKILL.md", skill_md("docx"))
        .write("dev/git/commit/SKILL.md", skill_md("commit"))
        .write(".git-like/x/SKILL.md", skill_md("x"));
    up.commit("v1");

    assert_eq!(
        candidates(&sandbox, "acme/grouped"),
        "x  (.git-like/x)\ncommit  (dev/git/commit)\ndocx  (office/docx)\npdf  (office/pdf)\n"
    );
}

#[test]
fn a_skill_md_nested_below_a_candidate_is_not_a_second_candidate() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = sandbox.upstream("acme/nested");
    up.write("skills/pdf/SKILL.md", skill_md("pdf"))
        .write("skills/pdf/templates/form/SKILL.md", skill_md("form"))
        .write("skills/docx/SKILL.md", skill_md("docx"));
    up.commit("v1");

    assert_eq!(
        candidates(&sandbox, "acme/nested"),
        "docx  (skills/docx)\npdf  (skills/pdf)\n"
    );
}

#[test]
fn a_candidate_without_a_frontmatter_name_goes_by_its_folder_name() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = sandbox.upstream("acme/skills");
    up.write("a/notes/SKILL.md", "---\ndescription: Notes\n---\n")
        .write(
            "b/pdf/SKILL.md",
            "---\nname: PDF\ndescription: Forms\n---\n",
        );
    up.commit("v1");

    assert_eq!(
        candidates(&sandbox, "acme/skills"),
        "notes  (a/notes)\nPDF  (b/pdf)\n"
    );
}

#[test]
fn a_source_without_any_skill_md_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = sandbox.upstream("acme/empty");
    up.write("README.md", "# nothing here\n");
    up.commit("v1");
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/empty", "--yes"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: no skill found in https://github.com/acme/empty.git: no folder holds a SKILL.md

"#]]);
    assert_eq!(world(&sandbox), before);
}

#[test]
fn skill_flags_install_the_named_skills_and_record_each_path() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = flat_repo(&sandbox);
    up.write("skills/xlsx/SKILL.md", skill_md("xlsx"));
    let commit = up.commit("v2");
    let repo = sandbox.skill_repo();

    sandbox
        .skiman()
        .args([
            "add",
            "acme/skills",
            "--skill",
            "pdf",
            "--skill",
            "xlsx",
            "--yes",
        ])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skill pdf from https://github.com/acme/skills.git (skills/pdf) at [..] (default branch main):
  SKILL.md
  reference.md
skill xlsx from https://github.com/acme/skills.git (skills/xlsx) at [..] (default branch main):
  SKILL.md
created [..]/skill-repo/upstream/pdf
created [..]/skill-repo/skills/pdf
created [..]/skill-repo/upstream/xlsx
created [..]/skill-repo/skills/xlsx
changed [..]/skill-repo/skiman.toml
created [..]/skill-repo/skiman.lock
...
"#]])
        .stderr_eq("");

    assert_eq!(
        read(repo.join("skills/pdf/reference.md")),
        "# pdf reference\n"
    );
    assert_eq!(read(repo.join("upstream/xlsx/SKILL.md")), skill_md("xlsx"));
    assert!(!repo.join("skills/docx").exists());
    assert!(read(repo.join("skiman.toml")).ends_with(
        "[skills.pdf.source]\nurl = \"https://github.com/acme/skills.git\"\npath = \"skills/pdf\"\n\n\
         [skills.xlsx.source]\nurl = \"https://github.com/acme/skills.git\"\npath = \"skills/xlsx\"\n"
    ));
    let lock = read(repo.join("skiman.lock"));
    assert_eq!(
        lock.matches(&format!("commit = \"{commit}\"")).count(),
        2,
        "{lock}"
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn all_installs_every_candidate() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    flat_repo(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/skills", "--all", "--yes"])
        .assert()
        .code(0);

    let repo = sandbox.skill_repo();
    for name in ["pdf", "docx"] {
        assert!(
            repo.join(format!("skills/{name}/SKILL.md")).is_file(),
            "{name}"
        );
        assert!(
            repo.join(format!("upstream/{name}/SKILL.md")).is_file(),
            "{name}"
        );
    }
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn without_a_terminal_a_multi_skill_source_needs_skill_or_all() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    flat_repo(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/skills", "--yes"])
        .assert()
        .code(2)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: missing flag without a terminal: https://github.com/acme/skills.git holds 2 skills; choose with --skill <name> (repeatable) or --all

"#]]);
    assert_eq!(world(&sandbox), before);
}

#[test]
fn an_unknown_skill_name_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    flat_repo(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args([
            "add",
            "acme/skills",
            "--skill",
            "pdf",
            "--skill",
            "pptx",
            "--yes",
        ])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
error: https://github.com/acme/skills.git has no skill named `pptx`; it holds: docx, pdf

"#]]);
    assert_eq!(world(&sandbox), before);
}

#[test]
fn name_with_more_than_one_selected_skill_is_a_usage_error() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    flat_repo(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/skills", "--all", "--name", "mine", "--yes"])
        .assert()
        .code(2)
        .stderr_eq(str![[r#"
error: --name works with a single skill only; install the others under their upstream names first

"#]]);
    assert_eq!(world(&sandbox), before);
}

#[test]
fn one_taken_name_among_the_selection_refuses_the_whole_add() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "docx", "-d", "My own docx skill"])
        .assert()
        .code(0);
    flat_repo(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/skills", "--all", "--yes"])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
error: the name `docx` is taken: skills/docx already exists; nothing was installed; install that skill on its own with `--skill <name> --name <new name>`

"#]]);
    assert_eq!(world(&sandbox), before);
    assert!(!sandbox.skill_repo().join("skills/pdf").exists());
}

#[test]
fn with_a_terminal_the_user_picks_from_the_candidates() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    flat_repo(&sandbox);

    let mut pty = sandbox.skiman_pty(["add", "acme/skills"]);
    let shown = pty.expect("Install which skills? Numbers separated by spaces, or `all`: ");
    assert!(shown.contains("1. docx  (skills/docx)"), "{shown}");
    assert!(shown.contains("2. pdf  (skills/pdf)"), "{shown}");
    pty.send_line("2");
    let shown = pty.expect("Install skill pdf? [y/N] ");
    assert!(shown.contains("reference.md"), "{shown}");
    pty.send_line("y");
    let (code, rest) = pty.wait();
    assert_eq!(code, 0, "{rest}");

    let repo = sandbox.skill_repo();
    assert!(repo.join("skills/pdf/SKILL.md").is_file());
    assert!(!repo.join("skills/docx").exists());
}

#[test]
fn a_tree_url_with_a_slash_branch_installs_that_folder_at_that_branch() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = flat_repo(&sandbox);
    up.switch("feature/x/y");
    up.write(
        "skills/pdf/SKILL.md",
        skill_md("pdf").replace("# pdf", "# pdf on feature"),
    );
    let feature = up.commit("feature work");
    up.switch("main");
    // A tag that matches a shorter prefix must lose to the longer branch.
    up.tag("feature");
    let repo = sandbox.skill_repo();

    sandbox
        .skiman()
        .args(["add", "--yes", &up.tree_url("feature/x/y", "skills/pdf")])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skill pdf from https://github.com/acme/skills.git (skills/pdf) at [..] (branch feature/x/y):
  SKILL.md
  reference.md
...
"#]]);

    assert!(read(repo.join("upstream/pdf/SKILL.md")).contains("# pdf on feature"));
    assert!(read(repo.join("skiman.toml")).ends_with(
        "[skills.pdf.source]\nurl = \"https://github.com/acme/skills.git\"\npath = \"skills/pdf\"\n\
         ref = \"feature/x/y\"\nref_kind = \"branch\"\n"
    ));
    let lock = read(repo.join("skiman.lock"));
    assert!(
        lock.contains(&format!("commit = \"{feature}\"\nref = \"feature/x/y\"\n")),
        "{lock}"
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn a_blob_url_to_skill_md_installs_its_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = flat_repo(&sandbox);
    let url = "https://github.com/acme/skills/blob/main/skills/docx/SKILL.md";

    sandbox
        .skiman()
        .args(["add", "--yes", url])
        .assert()
        .code(0);

    let repo = sandbox.skill_repo();
    assert_eq!(read(repo.join("skills/docx/SKILL.md")), skill_md("docx"));
    assert!(!repo.join("skills/pdf").exists());
    let toml = read(repo.join("skiman.toml"));
    assert!(
        toml.ends_with("path = \"skills/docx\"\nref = \"main\"\nref_kind = \"branch\"\n"),
        "{toml}"
    );
    assert!(read(repo.join("skiman.lock")).contains(&format!("commit = \"{}\"", up.head())));
}

#[test]
fn a_tree_url_whose_ref_matches_no_branch_or_tag_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = flat_repo(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "--yes", &up.tree_url("nope", "skills/pdf")])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
error: no branch or tag of https://github.com/acme/skills.git matches the start of `nope/skills/pdf`

"#]]);
    assert_eq!(world(&sandbox), before);
}

/// `acme/pdf` with three versions of one skill: a `v1` tag, a `stable`
/// branch and `main` ahead of both. Returns the upstream and the commits.
fn versioned(sandbox: &Sandbox) -> (Upstream, [String; 3]) {
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", skill_md("pdf"))
        .write("VERSION", "1\n");
    let one = up.commit("one");
    up.git(["tag", "-a", "-m", "release one", "v1"]);
    up.write("VERSION", "2\n");
    let two = up.commit("two");
    up.branch("stable");
    up.write("VERSION", "3\n");
    let three = up.commit("three");
    (up, [one, two, three])
}

#[test]
fn ref_installs_a_branch_a_tag_or_a_full_commit_and_records_its_kind() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let (_up, [one, two, three]) = versioned(&sandbox);
    let repo = sandbox.skill_repo();

    for (git_ref, kind, commit, version) in [
        ("stable", "branch", &two, "2\n"),
        ("v1", "tag", &one, "1\n"),
        (three.as_str(), "commit", &three, "3\n"),
    ] {
        sandbox
            .skiman()
            .args([
                "add",
                "acme/pdf",
                "--name",
                &format!("pdf-{kind}"),
                "--ref",
                git_ref,
                "--yes",
            ])
            .assert()
            .code(0);
        let name = format!("pdf-{kind}");
        assert_eq!(read(repo.join(format!("upstream/{name}/VERSION"))), version);
        let toml = read(repo.join("skiman.toml"));
        assert!(
            toml.contains(&format!(
                "[skills.{name}.source]\nurl = \"https://github.com/acme/pdf.git\"\npath = \"\"\n\
                 ref = \"{git_ref}\"\nref_kind = \"{kind}\"\nupstream_name = \"pdf\"\n"
            )),
            "{toml}"
        );
        let lock = read(repo.join("skiman.lock"));
        assert!(
            lock.contains(&format!(
                "[skills.{name}]\ncommit = \"{commit}\"\nref = \"{git_ref}\"\n"
            )),
            "{lock}"
        );
    }
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn ref_with_a_short_sha_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let (_up, [_, two, _]) = versioned(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--ref", &two[..12], "--yes"])
        .assert()
        .code(1)
        .stderr_eq(format!(
            "error: `{}` is not a branch or tag of https://github.com/acme/pdf.git; a commit must be given as its full 40-hex sha\n",
            &two[..12]
        ));
    assert_eq!(world(&sandbox), before);
}

#[test]
fn a_name_that_is_both_a_branch_and_a_tag_installs_the_branch_and_reports_the_tag() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let (up, [one, _, three]) = versioned(&sandbox);
    up.branch("release");
    up.git(["tag", "release", &one]);
    let repo = sandbox.skill_repo();

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--ref", "release", "--yes"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skill pdf from https://github.com/acme/pdf.git at [..] (branch release):
  note: `release` is also a tag; the branch is installed
  SKILL.md
  VERSION
...
"#]]);

    assert_eq!(read(repo.join("upstream/pdf/VERSION")), "3\n");
    assert!(read(repo.join("skiman.lock")).contains(&format!("commit = \"{three}\"")));
    assert!(read(repo.join("skiman.toml")).contains("ref_kind = \"branch\"\n"));
}

#[test]
fn ref_together_with_a_tree_url_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = flat_repo(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args([
            "add",
            "--yes",
            "--ref",
            "main",
            &up.tree_url("main", "skills/pdf"),
        ])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
error: the URL already names a ref; drop --ref, or give the repository URL with --ref

"#]]);
    assert_eq!(world(&sandbox), before);
}

/// A local folder `~/src/pdf` holding one skill and a `.git` folder.
fn local_upstream(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.home().join("src/pdf");
    std::fs::create_dir_all(&dir).unwrap();
    sandbox.git(&dir, ["init", "--quiet"]);
    std::fs::write(dir.join("SKILL.md"), skill_md("pdf")).unwrap();
    std::fs::create_dir_all(dir.join("lib/.git")).unwrap();
    std::fs::write(dir.join("lib/.git/config"), "nested\n").unwrap();
    std::fs::write(dir.join("lib/forms.md"), "# forms\n").unwrap();
    std::fs::write(dir.join(".DS_Store"), "junk").unwrap();
    dir
}

#[test]
fn a_local_folder_installs_without_git_content_and_without_a_commit() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = local_upstream(&sandbox);
    let repo = sandbox.skill_repo();

    sandbox
        .skiman()
        .args(["add", "~/src/pdf", "--yes"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skill pdf from ~/src/pdf (local folder):
  SKILL.md
  lib/forms.md
created [..]/skill-repo/upstream/pdf
...
"#]])
        .stderr_eq("");

    for root in ["skills/pdf", "upstream/pdf"] {
        let files: Vec<PathBuf> = tree(repo.join(root)).into_keys().collect();
        assert_eq!(
            files,
            [
                PathBuf::from("SKILL.md"),
                PathBuf::from("lib"),
                PathBuf::from("lib/forms.md")
            ],
            "{root}"
        );
    }
    assert!(
        read(repo.join("skiman.toml"))
            .ends_with("[skills.pdf.source]\nurl = \"~/src/pdf\"\npath = \"\"\n")
    );
    let lock = read(repo.join("skiman.lock"));
    assert!(
        lock.starts_with("version = 1\n\n[skills.pdf]\nsnapshot = \"sha256:"),
        "{lock}"
    );
    assert!(!lock.contains("commit"), "{lock}");
    assert!(
        dir.join(".git/HEAD").is_file(),
        "the source folder is untouched"
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn absolute_and_dot_relative_local_folders_are_recorded_as_absolute_paths() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = local_upstream(&sandbox);
    let repo = sandbox.skill_repo();

    sandbox
        .skiman()
        .args(["add", "./src/pdf", "--name", "pdf-dot", "--yes"])
        .assert()
        .code(0);
    sandbox
        .skiman()
        .arg("add")
        .arg(&dir)
        .args(["--name", "pdf-abs", "--yes"])
        .assert()
        .code(0);

    let toml = read(repo.join("skiman.toml"));
    for name in ["pdf-dot", "pdf-abs"] {
        assert!(
            toml.contains(&format!(
                "[skills.{name}.source]\nurl = \"{}\"\npath = \"\"\nupstream_name = \"pdf\"\n",
                dir.display()
            )),
            "{toml}"
        );
    }
}

#[test]
fn a_local_folder_with_several_skills_installs_the_selected_one() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = sandbox.home().join("src/many");
    for name in ["pdf", "docx"] {
        std::fs::create_dir_all(dir.join(name)).unwrap();
        std::fs::write(dir.join(name).join("SKILL.md"), skill_md(name)).unwrap();
    }
    // A symlink in a skill that is not selected does not stop the install.
    std::os::unix::fs::symlink("/etc/passwd", dir.join("docx/passwd")).unwrap();

    sandbox
        .skiman()
        .args(["add", "~/src/many", "--skill", "pdf", "--yes"])
        .assert()
        .code(0);

    let repo = sandbox.skill_repo();
    assert!(read(repo.join("skiman.toml")).ends_with("url = \"~/src/many\"\npath = \"pdf\"\n"));
    assert!(!repo.join("skills/docx").exists());
}

#[test]
fn a_local_folder_with_a_symlink_or_a_ref_or_that_is_missing_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = local_upstream(&sandbox);
    std::os::unix::fs::symlink("../../etc/passwd", dir.join("passwd")).unwrap();
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["add", "~/src/pdf", "--yes"])
        .assert()
        .code(1)
        .stderr_eq(format!(
            "error: {} holds a symlink at passwd; symlinks are not allowed in skills\n",
            dir.display()
        ));
    sandbox
        .skiman()
        .args(["add", "~/src/pdf", "--ref", "main", "--yes"])
        .assert()
        .code(1)
        .stderr_eq("error: ~/src/pdf is a local folder, which has no versions; drop --ref\n");
    sandbox
        .skiman()
        .args(["add", "~/src/gone", "--yes"])
        .assert()
        .code(1)
        .stderr_eq(format!(
            "error: the local folder {} does not exist\n",
            sandbox.home().join("src/gone").display()
        ));
    assert_eq!(world(&sandbox), before);
}

#[test]
fn without_a_terminal_the_users_ssh_command_gets_batch_mode_and_missing_credentials_exit_4() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let bin = sandbox.root().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let argv = sandbox.root().join("ssh-argv");
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho 'git@example.test: Permission denied (publickey).' >&2\nexit 255\n",
            argv.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut config = read(sandbox.git_config());
    config.push_str(&format!(
        "[core]\n\tsshCommand = {} -F /dev/null -o User=mine\n",
        ssh.display()
    ));
    std::fs::write(sandbox.git_config(), config).unwrap();
    let before = world(&sandbox);

    let output = sandbox
        .skiman()
        .args(["add", "git@example.test:acme/pdf.git", "--yes"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(4), "{stderr}");
    assert!(stderr.contains("Permission denied (publickey)"), "{stderr}");
    let argv: Vec<String> = read(&argv).lines().map(str::to_owned).collect();
    let host = argv
        .iter()
        .position(|a| a == "git@example.test")
        .expect("host in argv");
    let before_host = &argv[..host];
    for (option, value) in [
        ("-F", "/dev/null"),
        ("-o", "User=mine"),
        ("-o", "BatchMode=yes"),
    ] {
        assert!(
            before_host
                .windows(2)
                .any(|w| w[0] == option && w[1] == value),
            "{option} {value} missing before the host: {argv:?}"
        );
    }
    assert_eq!(world(&sandbox), before);
}

#[test]
fn a_fifo_named_skill_md_in_a_local_folder_is_never_opened() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = sandbox.home().join("src/many");
    std::fs::create_dir_all(dir.join("pdf")).unwrap();
    std::fs::write(dir.join("pdf/SKILL.md"), skill_md("pdf")).unwrap();
    std::fs::create_dir_all(dir.join("pipe")).unwrap();
    sandbox
        .command("mkfifo")
        .arg(dir.join("pipe/SKILL.md"))
        .assert()
        .code(0);

    sandbox
        .skiman()
        .args(["add", "~/src/many", "--skill", "pdf", "--yes"])
        .assert()
        .code(0);
    sandbox
        .skiman()
        .args(["add", "~/src/many", "--skill", "pipe", "--yes"])
        .assert()
        .code(1)
        .stderr_eq(format!(
            "error: {} holds SKILL.md, which is not a regular file (a FIFO, socket or device)\n",
            dir.join("pipe").display()
        ));
}
