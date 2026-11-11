//! `skiman add` refuses hostile upstream content (spec Source fetching,
//! "Path safety", and Merge semantics): nothing outside the skill repo is
//! written, no upstream code runs, and `--yes` bypasses none of it.

mod support;

use std::path::{Path, PathBuf};

use support::{Sandbox, Tool, Upstream, tree};

const SKILL_MD: &str = "---\nname: pdf\ndescription: Fill PDF forms\n---\n\n# pdf\n";
const MIB: usize = 1024 * 1024;

fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

/// The whole skill repo and every tool directory, to prove a command changed nothing.
fn world(sandbox: &Sandbox) -> Vec<std::collections::BTreeMap<PathBuf, String>> {
    let mut world = vec![tree(sandbox.skill_repo())];
    world.extend(Tool::ALL.iter().map(|t| tree(sandbox.tool_dir(*t))));
    world
}

/// Runs `skiman add <source> --yes` and asserts it exits 1 at fetch time
/// (before the file list, so not through the rules on the candidate output),
/// changes nothing and prints an error that names `path`. Returns stderr.
fn assert_refused(sandbox: &Sandbox, source: &str, path: &str) -> String {
    let before = world(sandbox);
    let output = sandbox
        .skiman()
        .args(["add", source, "--yes"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{source}: {stderr}");
    assert_eq!(
        stdout, "",
        "{source}: refused after the file list: {stderr}"
    );
    assert!(
        stderr.starts_with("error: ") && stderr.contains(path),
        "{source}: the error does not name {path:?}: {stderr}"
    );
    assert_eq!(world(sandbox), before, "{source}: something changed");
    stderr
}

/// Writes `content` to a scratch file and stores it as a blob in `up`.
fn blob(sandbox: &Sandbox, up: &Upstream, content: &[u8]) -> String {
    let file = sandbox.root().join("fixture-object");
    std::fs::write(&file, content).unwrap();
    up.git(["hash-object", "-w", file.to_str().unwrap()])
}

fn oid_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// A tree object holding exactly `entries` (`mode`, name bytes, oid), in
/// that order. Written with `--literally`, so git checks none of the names.
fn raw_tree(sandbox: &Sandbox, up: &Upstream, entries: &[(&str, &[u8], &str)]) -> String {
    let mut bytes = Vec::new();
    for (mode, name, oid) in entries {
        bytes.extend_from_slice(mode.as_bytes());
        bytes.push(b' ');
        bytes.extend_from_slice(name);
        bytes.push(0);
        bytes.extend_from_slice(&oid_bytes(oid));
    }
    let file = sandbox.root().join("fixture-object");
    std::fs::write(&file, bytes).unwrap();
    up.git([
        "hash-object",
        "-t",
        "tree",
        "--literally",
        "-w",
        file.to_str().unwrap(),
    ])
}

/// Points `main` at a new commit whose root tree is `tree`.
fn commit_tree(up: &Upstream, tree: &str) {
    let commit = up.git(["commit-tree", tree, "-m", "hostile"]);
    up.git(["update-ref", "refs/heads/main", &commit]);
}

/// `acme/pdf` whose root tree holds `SKILL.md` plus `entries`.
fn crafted_upstream(
    sandbox: &Sandbox,
    entries: impl FnOnce(&Upstream) -> Vec<(&'static str, Vec<u8>, String)>,
) -> Upstream {
    let up = sandbox.upstream("acme/pdf");
    let skill_md = blob(sandbox, &up, SKILL_MD.as_bytes());
    let mut all = vec![("100644", b"SKILL.md".to_vec(), skill_md)];
    all.extend(entries(&up));
    let entries: Vec<(&str, &[u8], &str)> = all
        .iter()
        .map(|(mode, name, oid)| (*mode, name.as_slice(), oid.as_str()))
        .collect();
    let root = raw_tree(sandbox, &up, &entries);
    commit_tree(&up, &root);
    up
}

/// `acme/pdf` committed with porcelain: `SKILL.md` plus `files`.
fn plain_upstream(sandbox: &Sandbox, files: &[(&str, Vec<u8>)]) -> Upstream {
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", SKILL_MD);
    for (path, content) in files {
        up.write(path, content);
    }
    up.commit("v1");
    up
}

/// A local folder `~/src/pdf` holding `SKILL.md`.
fn local_upstream(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.home().join("src/pdf");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), SKILL_MD).unwrap();
    dir
}

fn write(path: impl AsRef<Path>, content: impl AsRef<[u8]>) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn lfs_pointer() -> String {
    format!(
        "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize 123456\n",
        "4d7a".repeat(16)
    )
}

#[test]
fn a_dotdot_segment_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    crafted_upstream(&sandbox, |up| {
        let file = blob(&sandbox, up, b"escaped\n");
        let parent = raw_tree(&sandbox, up, &[("100644", b"escape.md", &file)]);
        vec![("40000", b"..".to_vec(), parent)]
    });

    assert_refused(&sandbox, "acme/pdf", "../escape.md");
}

#[test]
fn an_absolute_path_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    crafted_upstream(&sandbox, |up| {
        let file = blob(&sandbox, up, b"escaped\n");
        vec![("100644", b"/tmp/escape.md".to_vec(), file)]
    });

    assert_refused(&sandbox, "acme/pdf", "/tmp/escape.md");
}

#[test]
fn a_tree_entry_whose_name_holds_a_slash_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    crafted_upstream(&sandbox, |up| {
        let file = blob(&sandbox, up, b"hidden\n");
        vec![("100644", b"docs/hidden.md".to_vec(), file)]
    });

    assert_refused(&sandbox, "acme/pdf", "docs/hidden.md");
}

/// Names that would make the file list lie: a terminal escape, a carriage
/// return, a line feed, two direction controls and a line separator.
const DECEPTIVE_NAMES: [&str; 6] = [
    "steal.sh\u{1b}[8m",
    "run.sh\r  README.md",
    "a\nb.md",
    "notes\u{202e}hs.md",
    "x\u{61c}y.md",
    "line\u{2028}break.md",
];

#[test]
fn control_and_direction_changing_characters_in_a_name_are_refused_for_a_git_and_a_local_source() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    for name in DECEPTIVE_NAMES {
        crafted_upstream(&sandbox, |up| {
            let file = blob(&sandbox, up, b"#!/bin/sh\n");
            vec![("100755", name.as_bytes().to_vec(), file)]
        });
        let stderr = assert_refused(&sandbox, "acme/pdf", &format!("{name:?}"));
        assert!(!stderr.contains(name), "printed unescaped: {stderr}");
    }

    let dir = local_upstream(&sandbox);
    for name in DECEPTIVE_NAMES {
        write(dir.join("docs").join(name), "x\n");
        let stderr = assert_refused(
            &sandbox,
            "~/src/pdf",
            &format!("{:?}", format!("docs/{name}")),
        );
        assert!(!stderr.contains(name), "printed unescaped: {stderr}");
        std::fs::remove_file(dir.join("docs").join(name)).unwrap();
    }
}

/// Names in other scripts: Chinese, Arabic, a combining accent, and the
/// zero-width non-joiner Persian spelling needs.
const SCRIPT_NAMES: [&str; 4] = ["说明.md", "مرجع.md", "cafe\u{301}.md", "می\u{200c}خواهم.md"];

#[test]
fn names_in_other_scripts_and_with_combining_characters_are_installed_for_a_git_and_a_local_source()
{
    let sandbox = Sandbox::new();
    init(&sandbox);
    crafted_upstream(&sandbox, |up| {
        SCRIPT_NAMES
            .iter()
            .map(|name| {
                (
                    "100644",
                    name.as_bytes().to_vec(),
                    blob(&sandbox, up, b"x\n"),
                )
            })
            .collect()
    });
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);
    for name in SCRIPT_NAMES {
        assert!(
            sandbox.skill_repo().join("skills/pdf").join(name).is_file(),
            "{name}"
        );
    }

    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = local_upstream(&sandbox);
    for name in SCRIPT_NAMES {
        write(dir.join(name), "x\n");
    }
    sandbox
        .skiman()
        .args(["add", "~/src/pdf", "--yes"])
        .assert()
        .code(0);
    for name in SCRIPT_NAMES {
        assert!(
            sandbox.skill_repo().join("skills/pdf").join(name).is_file(),
            "{name}"
        );
    }
}

#[test]
fn a_macos_folder_icon_file_is_ignored_rather_than_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = local_upstream(&sandbox);
    write(dir.join("Icon\r"), "");

    sandbox
        .skiman()
        .args(["add", "~/src/pdf", "--yes"])
        .assert()
        .code(0);

    let skill = sandbox.skill_repo().join("skills/pdf");
    assert!(skill.join("SKILL.md").is_file());
    assert!(!skill.join("Icon\r").exists());
}

#[test]
fn a_candidate_folder_with_an_unsafe_name_is_skipped_and_the_others_can_be_installed() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = sandbox.upstream("acme/skills");
    up.write(
        "evil\nREADME.md/SKILL.md",
        "---\nname: evil\ndescription: Fakes a line\n---\n",
    )
    .write(
        "good/SKILL.md",
        "---\nname: good\ndescription: Does good things\n---\n",
    );
    up.commit("v1");

    let output = sandbox
        .skiman()
        .args(["add", "acme/skills", "--skill", "good", "--yes"])
        .output()
        .unwrap();

    // Read raw: snapbox would turn the escape's `\` into `/`.
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let (warning, rest) = stderr.split_once('\n').unwrap();
    assert!(
        warning.starts_with(
            r#"warning: https://github.com/acme/skills.git holds an unsafe path "evil\nREADME.md" ("#
        ) && warning.ends_with("; the skill there is skipped"),
        "{stderr}"
    );
    assert_eq!(rest, "");
    assert!(sandbox.skill_repo().join("skills/good/SKILL.md").is_file());
}

#[test]
fn a_case_collision_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    crafted_upstream(&sandbox, |up| {
        let upper = blob(&sandbox, up, b"upper\n");
        let lower = blob(&sandbox, up, b"lower\n");
        vec![
            ("100644", b"README.md".to_vec(), upper),
            ("100644", b"readme.md".to_vec(), lower),
        ]
    });

    let stderr = assert_refused(&sandbox, "acme/pdf", "README.md");
    assert!(stderr.contains("readme.md"), "{stderr}");
}

#[test]
fn a_unicode_normalisation_collision_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let nfc = "caf\u{e9}.md";
    let nfd = "cafe\u{301}.md";
    crafted_upstream(&sandbox, |up| {
        let one = blob(&sandbox, up, b"one\n");
        let two = blob(&sandbox, up, b"two\n");
        vec![
            ("100644", nfd.as_bytes().to_vec(), one),
            ("100644", nfc.as_bytes().to_vec(), two),
        ]
    });

    let stderr = assert_refused(&sandbox, "acme/pdf", nfc);
    assert!(stderr.contains(nfd), "{stderr}");
}

#[test]
fn a_git_lfs_pointer_is_refused_for_a_git_and_a_local_source() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    plain_upstream(&sandbox, &[("model.bin", lfs_pointer().into_bytes())]);
    let stderr = assert_refused(&sandbox, "acme/pdf", "model.bin");
    assert!(stderr.contains("Git LFS pointer"), "{stderr}");

    let dir = local_upstream(&sandbox);
    write(dir.join("data/model.bin"), lfs_pointer());
    assert_refused(&sandbox, "~/src/pdf", "data/model.bin");
}

#[test]
fn an_oversized_file_is_refused_for_a_git_and_a_local_source() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let big = vec![b'x'; 10 * MIB + 1];
    plain_upstream(&sandbox, &[("big.bin", big.clone())]);
    assert_refused(&sandbox, "acme/pdf", "big.bin");

    let dir = local_upstream(&sandbox);
    write(dir.join("big.bin"), &big);
    assert_refused(&sandbox, "~/src/pdf", "big.bin");
}

#[test]
fn a_skill_over_the_total_size_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    crafted_upstream(&sandbox, |up| {
        let part = blob(&sandbox, up, &vec![b'x'; 9 * MIB]);
        (1..=6)
            .map(|i| ("100644", format!("part{i}.bin").into_bytes(), part.clone()))
            .collect()
    });

    let stderr = assert_refused(&sandbox, "acme/pdf", "https://github.com/acme/pdf.git");
    assert!(stderr.contains("in total"), "{stderr}");
}

#[test]
fn too_many_files_are_refused_for_a_git_and_a_local_source() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let files: Vec<(String, Vec<u8>)> = (0..1000)
        .map(|i| (format!("many/{i}.md"), format!("{i}\n").into_bytes()))
        .collect();
    let refs: Vec<(&str, Vec<u8>)> = files.iter().map(|(p, c)| (p.as_str(), c.clone())).collect();
    plain_upstream(&sandbox, &refs);
    let stderr = assert_refused(&sandbox, "acme/pdf", "https://github.com/acme/pdf.git");
    assert!(stderr.contains("1001 files"), "{stderr}");

    let dir = local_upstream(&sandbox);
    for (path, content) in &files {
        write(dir.join(path), content);
    }
    let stderr = assert_refused(&sandbox, "~/src/pdf", &dir.display().to_string());
    assert!(stderr.contains("1001 files"), "{stderr}");
}

#[test]
fn a_symlink_is_refused_for_a_git_and_a_local_source() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = plain_upstream(&sandbox, &[]);
    up.symlink("docs/passwd", "../../../etc/passwd");
    up.commit("v2");
    assert_refused(&sandbox, "acme/pdf", "docs/passwd");

    let dir = local_upstream(&sandbox);
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", dir.join("docs/passwd")).unwrap();
    assert_refused(&sandbox, "~/src/pdf", "docs/passwd");
}

#[test]
fn a_special_file_in_a_local_source_is_refused_and_never_opened() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = local_upstream(&sandbox);
    std::fs::create_dir_all(dir.join("io")).unwrap();
    sandbox
        .command("mkfifo")
        .arg(dir.join("io/pipe"))
        .assert()
        .code(0);

    let stderr = assert_refused(&sandbox, "~/src/pdf", "io/pipe");
    assert!(stderr.contains("not a regular file"), "{stderr}");
}

#[test]
fn conflict_record_names_and_marker_lines_in_an_upstream_are_refused() {
    let cases: [(&str, &str, &[u8]); 5] = [
        (
            "notes.md.skiman-upstream",
            "notes.md.skiman-upstream",
            b"x\n",
        ),
        (
            "gone.md.skiman-upstream-deleted",
            "gone.md.skiman-upstream-deleted",
            b"",
        ),
        (".skiman-conflicts/notes.md", ".skiman-conflicts", b"x\n"),
        (
            "docs/notes.md",
            "docs/notes.md:3",
            b"a\nb\n<<<<<<< local (skiman)\nmine\n",
        ),
        ("notes.md", "notes.md:1", b">>>>>>> upstream (skiman)\n"),
    ];
    for (file, named, content) in cases {
        let sandbox = Sandbox::new();
        init(&sandbox);
        plain_upstream(&sandbox, &[(file, content.to_vec())]);
        assert_refused(&sandbox, "acme/pdf", named);
    }

    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = local_upstream(&sandbox);
    write(dir.join("notes.md.skiman-upstream"), "x\n");
    assert_refused(&sandbox, "~/src/pdf", "notes.md.skiman-upstream");
}

#[test]
fn filter_drivers_hook_templates_and_fsmonitor_from_the_users_git_config_never_run() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = plain_upstream(
        &sandbox,
        &[
            (
                ".gitattributes",
                b"* filter=evil diff=evil merge=evil\n".to_vec(),
            ),
            ("reference.md", b"# Reference\n".to_vec()),
        ],
    );
    drop(up);

    let markers = sandbox.root().join("markers");
    let script = |name: &str| {
        let path = sandbox.root().join(format!("bin/{name}"));
        write(
            &path,
            format!(
                "#!/bin/sh\nmkdir -p '{0}'\ntouch '{0}/{name}'\ncat\n",
                markers.display()
            ),
        );
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        path
    };
    let templates = sandbox.root().join("templates");
    for hook in [
        "reference-transaction",
        "post-checkout",
        "post-merge",
        "pre-auto-gc",
        "post-index-change",
    ] {
        let path = script(hook);
        write(
            templates.join("hooks").join(hook),
            std::fs::read(&path).unwrap(),
        );
        std::fs::set_permissions(
            templates.join("hooks").join(hook),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
    }
    let mut config = std::fs::read_to_string(sandbox.git_config()).unwrap();
    config.push_str(&format!(
        "[init]\n\ttemplateDir = {}\n\
         [filter \"evil\"]\n\tclean = {}\n\tsmudge = {}\n\tprocess = {}\n\trequired = true\n\
         [diff \"evil\"]\n\ttextconv = {}\n\
         [merge \"evil\"]\n\tdriver = {}\n\
         [core]\n\tfsmonitor = {}\n",
        templates.display(),
        script("clean").display(),
        script("smudge").display(),
        script("process").display(),
        script("textconv").display(),
        script("merge-driver").display(),
        script("fsmonitor").display(),
    ));
    std::fs::write(sandbox.git_config(), config).unwrap();

    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);

    let repo = sandbox.skill_repo();
    assert!(repo.join("upstream/pdf/reference.md").is_file());
    for dir in ["skills/pdf", "upstream/pdf"] {
        assert!(
            !repo.join(dir).join(".gitattributes").exists(),
            "the upstream's .gitattributes would apply to the skill repo"
        );
    }
    assert!(
        !markers.exists(),
        "upstream code ran: {:?}",
        std::fs::read_dir(&markers).map(|d| d.map(|e| e.unwrap().file_name()).collect::<Vec<_>>())
    );
}

/// Files that configure git, as an upstream may carry them.
const GIT_FILES: [(&str, &str); 3] = [
    (".gitignore", "*.log\n"),
    ("docs/.gitattributes", "*.md text\n"),
    (".gitmodules", "[submodule \"lib\"]\n\tpath = lib\n"),
];

#[test]
fn adopted_skills_and_project_skills_keep_their_own_git_files() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let foreign = sandbox.tool_dir(Tool::ClaudeCode).join("pdf");
    write(foreign.join("SKILL.md"), SKILL_MD);
    for (path, content) in GIT_FILES {
        write(foreign.join(path), content);
    }
    sandbox
        .skiman()
        .args(["adopt", "pdf", "--from", "claude-code", "--yes"])
        .assert()
        .code(0);
    for (path, content) in GIT_FILES {
        let copy = sandbox.skill_repo().join("skills/pdf").join(path);
        assert_eq!(std::fs::read_to_string(copy).unwrap(), content, "{path}");
    }

    let project = sandbox.home().join("work");
    let skill = project.join(".claude/skills/forms");
    write(
        skill.join("SKILL.md"),
        "---\nname: forms\ndescription: Fill forms\n---\n",
    );
    for (path, content) in GIT_FILES {
        write(skill.join(path), content);
    }
    sandbox
        .skiman()
        .args(["project", "add"])
        .arg(&project)
        .assert()
        .code(0);
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    let mirror = sandbox
        .skill_repo()
        .join("projects/work/.claude/skills/forms");
    for (path, content) in GIT_FILES {
        assert_eq!(
            std::fs::read_to_string(mirror.join(path)).unwrap(),
            content,
            "{path}"
        );
    }
}
