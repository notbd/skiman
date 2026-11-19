//! Path rules and the write guard (spec "Path rules"): which folders can be
//! registered, which child links count, and that skiman never writes
//! inside a registered folder by any road.

use std::os::unix::fs::symlink;
use std::path::Path;

use super::{COMPANY, append_manifest, contents, folder, sync};
use crate::support::{Sandbox, Tool, exists, init, read, skill_md, write};

fn linked_add(sandbox: &Sandbox, path: &Path) -> (i32, String) {
    let output = sandbox
        .skiman()
        .args(["linked", "add"])
        .arg(path)
        .output()
        .unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// The canon's `skiman.toml` and `.gitignore`: a refused `linked add`
/// leaves both as they were.
fn records(sandbox: &Sandbox) -> (String, String) {
    let repo = sandbox.canon();
    (
        read(repo.join("skiman.toml")),
        read(repo.join(".gitignore")),
    )
}

#[test]
fn linked_add_refuses_a_path_that_overlaps_a_place_skiman_writes_or_home() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    let home = sandbox.home();
    std::fs::create_dir_all(repo.join("skills/inner")).unwrap();
    let before = records(&sandbox);
    for (path, why) in [
        (repo.join("skills/inner"), "lies in"),
        (
            sandbox.root().to_owned(),
            "is your home folder or one of its parents",
        ),
        (home.clone(), "is your home folder or one of its parents"),
        (sandbox.tool_dir(Tool::Codex), "lies in"),
        (home.join(".agents"), "contains"),
    ] {
        std::fs::create_dir_all(&path).unwrap();
        let (code, stderr) = linked_add(&sandbox, &path);
        assert_eq!(code, 1, "{}: {stderr}", path.display());
        assert!(stderr.contains(why), "{}: {stderr}", path.display());
        assert_eq!(records(&sandbox), before);
    }
    let (code, stderr) = linked_add(&sandbox, &home.join("not-there"));
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("is not an existing folder"), "{stderr}");
    std::fs::write(home.join("file"), "x").unwrap();
    let (code, stderr) = linked_add(&sandbox, &home.join("file"));
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("is not a folder"), "{stderr}");
    assert_eq!(records(&sandbox), before);
}

/// A link an older script left in a tool directory is no way to name a
/// linked folder: the path goes through a place skiman writes. The error
/// names the realpath to register instead.
#[test]
fn a_path_through_a_link_in_a_tool_directory_is_refused_with_no_record_left() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    let old = sandbox.tool_dir(Tool::ClaudeCode).join("company");
    symlink(&company, &old).unwrap();
    let alias = sandbox.home().join("alias");
    symlink(&old, &alias).unwrap();
    let before = records(&sandbox);

    for path in [&old, &alias] {
        let (code, stderr) = linked_add(&sandbox, path);
        assert_eq!(code, 1, "{stderr}");
        assert!(
            stderr.contains(&format!(
                "register its realpath {} instead",
                company.display()
            )),
            "{stderr}"
        );
        assert_eq!(records(&sandbox), before);
    }
    assert_eq!(linked_add(&sandbox, &company).0, 0);
}

/// A link whose target goes through a file and `..` names nothing, as the
/// system refuses to resolve it: `linked add` refuses it and records nothing.
#[test]
fn a_path_through_a_file_and_dot_dot_is_not_an_existing_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let home = sandbox.home();
    folder(&sandbox, "target", &["alpha"]);
    write(home.join("file"), "x");
    symlink("file/../target", home.join("alias")).unwrap();
    let before = records(&sandbox);

    let (code, stderr) = linked_add(&sandbox, &home.join("alias"));

    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("is not an existing folder"), "{stderr}");
    assert_eq!(records(&sandbox), before);
    assert!(!exists(sandbox.canon().join("linked/alias")));
}

/// A path that reaches a registered folder only through a link on the way
/// overlaps it all the same: `linked add` refuses it whichever id sorts
/// first, and the registered folder keeps distributing its child link.
#[test]
fn linked_add_refuses_a_path_through_a_link_inside_a_registered_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    let outside = sandbox.root().join("outside");
    write(outside.join("SKILL.md"), skill_md("bridge"));
    symlink(&outside, company.join("bridge")).unwrap();
    let indirect = sandbox.home().join("indirect");
    symlink(&company, &indirect).unwrap();
    append_manifest(&sandbox, COMPANY);
    assert_eq!(sync(&sandbox).0, 0);
    let before = records(&sandbox);

    for id in ["aaa", "zzz"] {
        let output = sandbox
            .skiman()
            .args(["linked", "add", "--id", id])
            .arg(indirect.join("bridge"))
            .output()
            .unwrap();

        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(1), "{id}: {stderr}");
        assert!(stderr.contains("overlaps linked folder"), "{id}: {stderr}");
        assert_eq!(records(&sandbox), before, "{id}");
    }
    assert_eq!(
        std::fs::read_link(sandbox.tool_dir(Tool::ClaudeCode).join("bridge")).unwrap(),
        sandbox.canon().join("linked/company/bridge")
    );
}

/// Of two registrations that overlap, the one whose id sorts first keeps
/// working; a table that is not well formed is skipped too (exit 3).
#[test]
fn overlapping_and_malformed_registrations_break_the_rules() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    folder(&sandbox, "work/company", &["alpha", "beta"]);
    append_manifest(
        &sandbox,
        "\n[linked.zinner]\npath = \"~/work/company/alpha\"\n\n[linked.company]\npath = \"~/work/company\"\n\n[linked.bad]\npath = \"work\"\n",
    );

    let (code, stderr) = sync(&sandbox);

    assert_eq!(code, 3, "{stderr}");
    assert!(
        stderr.contains("linked folder `zinner` is skipped: ")
            && stderr.contains("overlaps linked folder `company`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("linked folder `bad` is skipped: `path` in [linked.bad] must be"),
        "{stderr}"
    );
    let linked = sandbox.canon().join("linked");
    assert!(!exists(linked.join("zinner")) && !exists(linked.join("bad")));
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert_eq!(
            std::fs::read_link(dir.join("alpha")).unwrap(),
            linked.join("company/alpha")
        );
    }
}

/// A table that is not well formed (an unknown key, an invalid id) is
/// skipped, in the words `check` uses, but its path keeps the protection:
/// sync writes nothing through a tool directory that resolves into the
/// folder and gives an older script's link into it no adopt hint (nor the
/// hint for a folder skiman distributes), and adopt refuses that link.
#[test]
fn a_malformed_registration_still_protects_its_folder() {
    for (id, table, why) in [
        (
            "company",
            "[linked.company]\npath = \"~/work/company\"\nnote = \"x\"\n",
            "unknown key `note` in [linked.company] (allowed: path)",
        ),
        (
            "Company",
            "[linked.Company]\npath = \"~/work/company\"\n",
            "`Company` is not a valid linked folder id: use lowercase letters, digits and single hyphens",
        ),
    ] {
        let sandbox = Sandbox::new();
        init(&sandbox);
        let company = folder(&sandbox, "work/company", &["pdf"]);
        append_manifest(&sandbox, &format!("\n{table}"));
        let claude = sandbox.tool_dir(Tool::ClaudeCode);
        symlink(company.join("pdf"), claude.join("old")).unwrap();
        let codex = sandbox.tool_dir(Tool::Codex);
        std::fs::remove_dir_all(&codex).unwrap();
        std::fs::create_dir(company.join("agents-skills")).unwrap();
        symlink(company.join("agents-skills"), &codex).unwrap();
        let before = contents(&[&company]);

        let (code, stderr) = sync(&sandbox);

        assert_eq!(code, 3, "{stderr}");
        let skipped = format!(
            "warning: linked folder `{id}` is skipped: {why}; fix [linked.{id}] in skiman.toml"
        );
        assert!(stderr.contains(&skipped), "{skipped}\n{stderr}");
        let line = format!(
            "warning: {} resolves to {}, inside linked folder `{id}`",
            codex.display(),
            company.join("agents-skills").display()
        );
        assert!(stderr.contains(&line), "{line}\n{stderr}");
        assert!(!stderr.contains("skiman adopt old"), "{stderr}");
        // Nor that skiman distributes the folder: it is skipped.
        let line = format!(
            "warning: {} is a link into linked folder `{id}`\n",
            claude.join("old").display()
        );
        assert!(stderr.contains(&line), "{line}\n{stderr}");

        let canon = crate::support::tree(sandbox.canon());
        let (code, stderr) = run(
            &sandbox,
            &["adopt", "old", "--from", "claude-code", "--yes"],
        );
        assert_eq!(code, 1, "{stderr}");
        let line = format!(
            "{} resolves into linked folder `{id}`",
            claude.join("old").display()
        );
        assert!(stderr.contains(&line), "{line}\n{stderr}");
        assert_eq!(crate::support::tree(sandbox.canon()), canon);
        assert_eq!(contents(&[&company]), before);
    }
}

/// Child links that point at `~`, at a tool directory or at an entry of
/// one break the path rules: each is reported and not distributed, and
/// every tool directory syncs as usual. A child link to an independent
/// folder elsewhere is a linked skill.
#[test]
fn child_links_into_home_or_a_tool_directory_are_not_distributed() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(
        sandbox.canon().join("skills/notes/SKILL.md"),
        skill_md("notes"),
    );
    assert_eq!(sync(&sandbox).0, 0);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    let independent = sandbox.root().join("independent/tool");
    write(independent.join("SKILL.md"), skill_md("tool"));
    let home = sandbox.home();
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write(home.join("SKILL.md"), skill_md("home"));
    for (name, target) in [
        ("home", home.clone()),
        ("skills", claude.clone()),
        ("notes-again", claude.join("notes")),
        ("tool", independent.clone()),
    ] {
        symlink(target, company.join(name)).unwrap();
    }
    append_manifest(&sandbox, COMPANY);
    let before = contents(&[&company, &independent]);

    let (code, stderr) = sync(&sandbox);

    assert_eq!(code, 0, "{stderr}");
    for name in ["home", "skills", "notes-again"] {
        let line = format!(
            "warning: {} is a link that breaks the path rules",
            company.join(name).display()
        );
        assert!(stderr.contains(&line), "{line}\n{stderr}");
    }
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        for name in ["home", "skills", "notes-again"] {
            assert!(!exists(dir.join(name)), "{name}");
        }
        for name in ["notes", "alpha", "tool"] {
            assert!(dir.join(name).join("SKILL.md").is_file(), "{name}");
        }
    }
    assert_eq!(contents(&[&company, &independent]), before);
}

/// A tool directory turned into a link into a registered folder after the
/// registration: sync writes nothing there (exit 3), the rest syncs, and
/// the folder stays byte for byte as it was.
#[test]
fn a_tool_directory_that_resolves_into_a_linked_folder_is_skipped() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(
        sandbox.canon().join("skills/notes/SKILL.md"),
        skill_md("notes"),
    );
    let company = folder(&sandbox, "work/company", &["alpha"]);
    append_manifest(&sandbox, COMPANY);
    assert_eq!(sync(&sandbox).0, 0);
    let codex = sandbox.tool_dir(Tool::Codex);
    std::fs::remove_dir_all(&codex).unwrap();
    std::fs::create_dir(company.join("agents-skills")).unwrap();
    symlink(company.join("agents-skills"), &codex).unwrap();
    let before = contents(&[&company]);

    let (code, stderr) = sync(&sandbox);

    assert_eq!(code, 3, "{stderr}");
    let line = format!(
        "warning: {} resolves to {}, inside linked folder `company`",
        codex.display(),
        company.join("agents-skills").display()
    );
    assert!(stderr.contains(&line), "{line}\n{stderr}");
    assert_eq!(contents(&[&company]), before);
    assert!(
        sandbox
            .tool_dir(Tool::ClaudeCode)
            .join("notes/SKILL.md")
            .is_file()
    );
}

/// A presence directory that holds skiman's instruction link and its
/// session-start hook, turned into a link into a registered folder with
/// its tool disabled: neither sync's cleanup nor `init --session-hooks
/// none` writes there (exit 3), and the folder stays byte for byte.
#[test]
fn a_presence_directory_that_resolves_into_a_linked_folder_is_left_alone() {
    let sandbox = Sandbox::new();
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "claude-code"])
        .arg(sandbox.canon())
        .assert()
        .code(0);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    append_manifest(&sandbox, COMPANY);
    assert_eq!(sync(&sandbox).0, 0);
    let claude = sandbox.presence_dir(Tool::ClaudeCode);
    std::fs::rename(&claude, company.join("claude")).unwrap();
    symlink(company.join("claude"), &claude).unwrap();
    let manifest = sandbox.canon().join("skiman.toml");
    std::fs::write(&manifest, read(&manifest).replace("\"claude-code\", ", "")).unwrap();
    assert!(
        std::fs::symlink_metadata(company.join("claude/CLAUDE.md"))
            .unwrap()
            .is_symlink()
    );
    assert!(read(company.join("claude/settings.json")).contains("sync --hook"));
    let before = contents(&[&company]);

    let (code, stderr) = sync(&sandbox);
    assert_eq!(code, 3, "{stderr}");
    assert!(
        stderr.contains("inside linked folder `company`"),
        "{stderr}"
    );
    // The folder now holds a presence directory: the registration breaks
    // the rules, and its protection holds all the same.
    assert!(
        stderr.contains("linked folder `company` is skipped: "),
        "{stderr}"
    );
    assert_eq!(contents(&[&company]), before);

    let output = sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.canon())
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("its session-start hook entry is left as it is"),
        "{stderr}"
    );
    assert_eq!(contents(&[&company]), before);
}

/// A canon `.gitignore` that is a link into a registered folder: `init`
/// writes nothing through it (exit 3).
#[test]
fn init_leaves_a_gitignore_that_resolves_into_a_linked_folder_alone() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    append_manifest(&sandbox, COMPANY);
    let gitignore = sandbox.canon().join(".gitignore");
    std::fs::remove_file(&gitignore).unwrap();
    std::fs::write(company.join("gitignore"), "target\n").unwrap();
    symlink(company.join("gitignore"), &gitignore).unwrap();
    let before = contents(&[&company]);

    let output = sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.canon())
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("inside linked folder `company`"),
        "{stderr}"
    );
    assert_eq!(contents(&[&company]), before);
}

/// A settings file that is a link into a registered folder when `init`
/// reads it and is pointed elsewhere before the write: the guard checks
/// the file that is written, so nothing is written into the folder.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_settings_file_relinked_before_the_write_is_guarded_where_it_is_written() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    append_manifest(&sandbox, COMPANY);
    write(company.join("settings.json"), "{}\n");
    let elsewhere = sandbox.root().join("elsewhere.json");
    write(&elsewhere, "{}\n");
    let settings = sandbox.presence_dir(Tool::ClaudeCode).join("settings.json");
    symlink(company.join("settings.json"), &settings).unwrap();
    let before = contents(&[&company]);

    let output = sandbox
        .skiman()
        .args(["init", "--session-hooks", "claude-code"])
        .arg(sandbox.canon())
        .env("SKIMAN_TEST_HOOK", "session-hooks:guard")
        .env(
            "SKIMAN_TEST_HOOK_RUN",
            format!("ln -sfn '{}' '{}'", elsewhere.display(), settings.display()),
        )
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("inside linked folder `company`"),
        "{stderr}"
    );
    assert_eq!(contents(&[&company]), before);
}

/// While `skiman.toml` cannot be parsed no registration is known, so
/// `init` writes nothing through a `.gitignore` that is a link.
#[test]
fn init_writes_no_linked_gitignore_while_the_manifest_does_not_parse() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    let real = sandbox.root().join("dotfiles/gitignore");
    write(&real, "target\n");
    std::fs::remove_file(repo.join(".gitignore")).unwrap();
    symlink(&real, repo.join(".gitignore")).unwrap();
    std::fs::write(repo.join("skiman.toml"), "[tools\n").unwrap();

    let output = sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&repo)
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_ne!(output.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("`.skiman/` is not added to it"), "{stderr}");
    assert_eq!(read(&real), "target\n");
}

/// `init` into a registered folder of the current canon, or into a folder
/// inside one, refuses before writing anything (exit 1): the user config
/// still names the current canon.
#[test]
fn init_refuses_a_folder_inside_a_linked_folder_of_the_current_canon() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &[]);
    append_manifest(&sandbox, COMPANY);
    assert_eq!(sync(&sandbox).0, 0);
    let config = sandbox.config_home().join("skiman");
    let before = (contents(&[&company]), crate::support::tree(&config));

    for target in [company.clone(), company.join("new")] {
        let output = sandbox
            .skiman()
            .args(["init", "--session-hooks", "none"])
            .arg(&target)
            .output()
            .unwrap();

        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(
            stderr.contains("lies inside, linked folder `company`")
                && stderr.contains("skiman linked remove company"),
            "{stderr}"
        );
        assert_eq!(
            (contents(&[&company]), crate::support::tree(&config)),
            before
        );
    }
}

/// A new registration that would take the place of an existing one (its id
/// sorts first and the folders overlap) is refused: the existing one keeps
/// working and nothing is written.
#[test]
fn linked_add_refuses_a_folder_that_would_break_an_existing_registration() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha", "beta"]);
    append_manifest(
        &sandbox,
        "\n[linked.zeta]\npath = \"~/work/company/beta\"\n",
    );
    assert_eq!(sync(&sandbox).0, 0);
    let before = records(&sandbox);

    let output = sandbox
        .skiman()
        .args(["linked", "add", "--id", "alpha"])
        .arg(&company)
        .output()
        .unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("it would break linked folder `zeta`"),
        "{stderr}"
    );
    assert_eq!(records(&sandbox), before);
    assert!(
        sandbox
            .tool_dir(Tool::ClaudeCode)
            .join("zeta/SKILL.md")
            .is_file()
    );
}

/// Runs skiman with `args` and returns its exit code and stderr.
fn run(sandbox: &Sandbox, args: &[&str]) -> (i32, String) {
    let output = sandbox.skiman().args(args).output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// An older script's link under another name that resolves into a
/// registered folder, or into the target of one of its child links: adopt
/// would copy linked content into the canon, so it refuses and copies
/// nothing.
#[test]
fn adopt_refuses_an_old_link_that_resolves_into_a_linked_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    let ext = sandbox.root().join("ext/tool");
    write(ext.join("SKILL.md"), skill_md("tool"));
    symlink(&ext, company.join("tool")).unwrap();
    append_manifest(&sandbox, COMPANY);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    symlink(company.join("alpha"), claude.join("old-alpha")).unwrap();
    symlink(&ext, claude.join("old-tool")).unwrap();
    assert_eq!(sync(&sandbox).0, 0);
    let canon = crate::support::tree(sandbox.canon());
    let before = contents(&[&company, &ext, &claude]);

    for name in ["old-alpha", "old-tool"] {
        let (code, stderr) = run(&sandbox, &["adopt", name, "--from", "claude-code", "--yes"]);
        assert_eq!(code, 1, "{stderr}");
        let line = format!(
            "{} resolves into linked folder `company`",
            claude.join(name).display()
        );
        assert!(stderr.contains(&line), "{line}\n{stderr}");
        assert_eq!(crate::support::tree(sandbox.canon()), canon);
        assert_eq!(contents(&[&company, &ext, &claude]), before);
    }
}

/// While a registered folder cannot be listed its child links are
/// unknown, so adopt refuses any entry.
#[test]
fn adopt_refuses_while_a_linked_folder_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    append_manifest(&sandbox, COMPANY);
    write(
        sandbox.tool_dir(Tool::ClaudeCode).join("beta/SKILL.md"),
        skill_md("beta"),
    );
    let canon = crate::support::tree(sandbox.canon());
    let chmod = |mode| std::fs::set_permissions(&company, std::fs::Permissions::from_mode(mode));

    chmod(0o000).unwrap();
    let (code, stderr) = run(
        &sandbox,
        &["adopt", "beta", "--from", "claude-code", "--yes"],
    );
    chmod(0o755).unwrap();

    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.contains("linked folder `company` cannot be read"),
        "{stderr}"
    );
    assert_eq!(crate::support::tree(sandbox.canon()), canon);
    let (code, stderr) = run(
        &sandbox,
        &["adopt", "beta", "--from", "claude-code", "--yes"],
    );
    assert_eq!(code, 0, "{stderr}");
}
