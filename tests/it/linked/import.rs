//! `skiman import` with linked folders (spec "Import"): the canon's
//! registrations survive, an import never collides with a linked skill,
//! and nothing is moved out of a linked folder or written into one.

use std::collections::BTreeMap;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use super::{COMPANY, append_manifest, contents, folder};
use crate::support::{Sandbox, Tool, backup_dirs, import, read, setup, skill_md, tree, write};

/// The canon, the machine state, every presence and tool directory, and
/// the linked folder `company`: a refused import changes none of them.
fn world(sandbox: &Sandbox) -> Vec<BTreeMap<PathBuf, String>> {
    let mut world = vec![
        tree(sandbox.canon()),
        tree(sandbox.state_home()),
        tree(sandbox.home().join(".agents")),
        tree(sandbox.home().join("work/company")),
    ];
    world.extend(Tool::ALL.iter().map(|t| tree(sandbox.presence_dir(*t))));
    world
}

/// Runs the import; asserts it is refused (exit 1) with `has` in its
/// message and changes nothing.
fn assert_refused(sandbox: &Sandbox, folder: &Path, has: &str) {
    let before = world(sandbox);
    let output = import(sandbox, folder);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains(has), "{has}\n{stderr}");
    assert_eq!(world(sandbox), before);
}

/// Runs the import; asserts its exit code and returns its stdout.
fn imported(sandbox: &Sandbox, folder: &Path, code: i32) -> String {
    let output = import(sandbox, folder);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(code), "{stderr}");
    String::from_utf8(output.stdout).unwrap()
}

fn target(sandbox: &Sandbox, tool: Tool, name: &str) -> PathBuf {
    std::fs::read_link(sandbox.tool_dir(tool).join(name)).unwrap()
}

/// Every entry in the backups folder, by its path below that folder.
fn backed_up(sandbox: &Sandbox) -> BTreeMap<PathBuf, String> {
    backup_dirs(sandbox)
        .iter()
        .flat_map(|dir| tree(dir).into_iter())
        .collect()
}

/// A full import keeps the canon's `[linked]` tables as it keeps its
/// project roots, and its closing sync links the linked skills; the links
/// sync made for them stay where they are, so none reaches the backup.
/// Running it again keeps them too.
#[test]
fn import_keeps_the_canons_linked_folders_and_links_their_skills() {
    let sandbox = Sandbox::new();
    let setup = setup(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    append_manifest(&sandbox, COMPANY);
    sandbox.skiman().arg("sync").assert().code(3);
    let linked = sandbox.canon().join("linked/company/alpha");
    assert_eq!(target(&sandbox, Tool::Codex, "alpha"), linked);
    let before = contents(&[&company]);

    let stdout = imported(&sandbox, &setup.folder, 0);

    let manifest = read(sandbox.canon().join("skiman.toml"));
    assert!(manifest.contains(COMPANY), "{manifest}");
    assert!(!stdout.contains("/alpha"), "{stdout}");
    for tool in Tool::ALL {
        assert_eq!(target(&sandbox, tool, "alpha"), linked);
    }
    assert!(
        !backed_up(&sandbox)
            .keys()
            .any(|path| path.ends_with("alpha")),
        "{:?}",
        backed_up(&sandbox)
    );

    // A skill added to the import folder makes the next run a full import again.
    write(setup.folder.join("skills/todo/SKILL.md"), skill_md("todo"));
    imported(&sandbox, &setup.folder, 0);
    let manifest = read(sandbox.canon().join("skiman.toml"));
    assert!(manifest.contains(COMPANY), "{manifest}");
    assert_eq!(target(&sandbox, Tool::ClaudeCode, "alpha"), linked);
    assert_eq!(contents(&[&company]), before);
}

/// An import brings skills and instructions only: an import folder whose
/// `skiman.toml` registers linked folders is refused before any write, with
/// the command that registers each once the import is done.
#[test]
fn an_import_folder_with_linked_tables_is_refused_before_any_write() {
    let sandbox = Sandbox::new();
    let setup = setup(&sandbox);
    let manifest = setup.folder.join("skiman.toml");
    write(
        &manifest,
        format!(
            "{}\n[linked.work]\npath = \"~/work/company\"\n",
            read(&manifest)
        ),
    );

    assert_refused(
        &sandbox,
        &setup.folder,
        "skiman linked add ~/work/company --id work",
    );
}

/// A skill of the import folder that uses a linked skill's name is refused
/// before any write, naming the linked folder.
#[test]
fn an_import_skill_that_uses_a_linked_skills_name_is_refused_before_any_write() {
    let sandbox = Sandbox::new();
    let setup = setup(&sandbox);
    let company = folder(&sandbox, "work/company", &["notes"]);
    append_manifest(&sandbox, COMPANY);

    assert_refused(
        &sandbox,
        &setup.folder,
        &format!(
            "skills/notes: it is a linked skill in {}, in linked folder `company`",
            company.join("notes").display()
        ),
    );
}

/// A group folder of the import folder takes its name like a skill does
/// (ADR 0009), at any depth: one named like a linked skill is refused
/// before any write.
#[test]
fn an_import_group_folder_that_uses_a_linked_skills_name_is_refused_before_any_write() {
    let sandbox = Sandbox::new();
    let setup = setup(&sandbox);
    let company = folder(&sandbox, "work/company", &["office"]);
    append_manifest(&sandbox, COMPANY);
    let skills = setup.folder.join("skills");
    std::fs::create_dir_all(skills.join("tools/office")).unwrap();
    std::fs::rename(skills.join("notes"), skills.join("tools/office/notes")).unwrap();

    assert_refused(
        &sandbox,
        &setup.folder,
        &format!(
            "skills/tools/office: it is a linked skill in {}, in linked folder `company`",
            company.join("office").display()
        ),
    );
}

/// While a registered folder cannot be read, what in the tool directories
/// leads into it is unknown: the import is refused before any write.
#[test]
fn import_refuses_while_a_linked_folder_cannot_be_read() {
    let sandbox = Sandbox::new();
    let setup = setup(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    append_manifest(&sandbox, COMPANY);
    let chmod = |mode| std::fs::set_permissions(&company, std::fs::Permissions::from_mode(mode));

    chmod(0o000).unwrap();
    let before = world(&sandbox);
    let output = import(&sandbox, &setup.folder);
    chmod(0o755).unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("linked folder `company` cannot be read now"),
        "{stderr}"
    );
    chmod(0o000).unwrap();
    let after = world(&sandbox);
    chmod(0o755).unwrap();
    assert_eq!(after, before);
}

/// A link an older script made to a linked skill's folder is a foreign
/// entry: the import moves the link to the backup and its closing sync
/// links the linked skill in its place; the folder stays as it was.
#[test]
fn import_takes_over_an_old_link_to_a_linked_skills_folder() {
    let sandbox = Sandbox::new();
    let setup = setup(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    symlink(company.join("alpha"), claude.join("alpha")).unwrap();
    append_manifest(&sandbox, COMPANY);
    let before = contents(&[&company]);

    let stdout = imported(&sandbox, &setup.folder, 0);

    assert!(
        stdout.contains(&format!(
            "move {} to the backup, then link it to skill alpha",
            claude.join("alpha").display()
        )),
        "{stdout}"
    );
    assert_eq!(
        target(&sandbox, Tool::ClaudeCode, "alpha"),
        sandbox.canon().join("linked/company/alpha")
    );
    let backup = backed_up(&sandbox);
    assert_eq!(
        backup.get(Path::new("claude-code/alpha")),
        Some(&format!("link -> {}", company.join("alpha").display())),
        "{backup:?}"
    );
    assert_eq!(contents(&[&company]), before);
}

/// A presence directory turned into a link into a registered folder: the
/// import would write through it into the folder, so it is refused before
/// any write, though the registration itself now breaks the rules.
#[test]
fn a_presence_directory_that_resolves_into_a_linked_folder_refuses_the_import() {
    let sandbox = Sandbox::new();
    let setup = setup(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    append_manifest(&sandbox, COMPANY);
    let claude = sandbox.presence_dir(Tool::ClaudeCode);
    std::fs::rename(&claude, company.join("claude")).unwrap();
    symlink(company.join("claude"), &claude).unwrap();

    assert_refused(&sandbox, &setup.folder, "inside linked folder `company`");
}

/// An instruction location that is a real folder holding a registered
/// path is not moved, so its content stays where it is; the other entries
/// are taken over. (A valid child target can lie in no presence directory,
/// so a registration that breaks the rules stands in for one: it is
/// protected all the same.)
#[test]
fn import_moves_no_instruction_location_that_holds_a_linked_folder() {
    let sandbox = Sandbox::new();
    let setup = setup(&sandbox);
    let location = sandbox.instruction_file(Tool::Codex);
    std::fs::remove_file(&location).unwrap();
    let inner = location.join("inner");
    write(inner.join("beta/SKILL.md"), skill_md("beta"));
    append_manifest(
        &sandbox,
        "\n[linked.inner]\npath = \"~/.codex/AGENTS.md/inner\"\n",
    );
    let before = contents(&[&location]);

    let stdout = imported(&sandbox, &setup.folder, 3);

    assert!(
        stdout.contains(&format!(
            "keep {}: it holds or lies in linked folder `inner`",
            location.display()
        )),
        "{stdout}"
    );
    assert_eq!(contents(&[&location]), before);
    let backup = backed_up(&sandbox);
    for moved in [
        "claude-code/CLAUDE.md",
        "claude-code/notes",
        "claude-code/old-copy",
    ] {
        assert!(backup.contains_key(Path::new(moved)), "{moved}: {backup:?}");
    }
    assert!(!backup.keys().any(|p| p.starts_with("codex/AGENTS.md")));
}

/// A tool directory that resolves into a registered folder refuses the
/// import before any write even when the import disables that tool: the
/// closing sync would still remove skiman's links there.
#[test]
fn a_disabled_tools_directory_that_resolves_into_a_linked_folder_refuses_the_import() {
    let sandbox = Sandbox::new();
    let setup = setup(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    append_manifest(&sandbox, COMPANY);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    std::fs::rename(&claude, company.join("claude-skills")).unwrap();
    symlink(company.join("claude-skills"), &claude).unwrap();
    let manifest = setup.folder.join("skiman.toml");
    std::fs::write(&manifest, read(&manifest).replace("\"claude-code\", ", "")).unwrap();

    assert_refused(&sandbox, &setup.folder, "inside linked folder `company`");
}

/// A tool directory that is a link to a plain folder, re-pointed into a
/// registered folder right before import moves an entry out of it: the
/// move takes the entry from the folder import checked, and the
/// registered folder stays byte for byte.
#[test]
fn import_moves_an_entry_out_of_the_tool_directory_it_checked() {
    let sandbox = Sandbox::new();
    let setup = setup(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    write(company.join("old-copy/notes.md"), "kept\n");
    append_manifest(&sandbox, COMPANY);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    let plain = sandbox.home().join("plain");
    std::fs::rename(&claude, &plain).unwrap();
    symlink(&plain, &claude).unwrap();
    let before = contents(&[&company]);
    let run = format!(
        "rm '{0}' && ln -s '{1}' '{0}'",
        claude.display(),
        company.display()
    );

    let output = crate::support::import_with_hook(
        &sandbox,
        &setup.folder,
        "import:back-up:claude-code/old-copy",
        &run,
    );

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(contents(&[&company]), before, "{stderr}");
    assert!(!plain.join("old-copy").exists(), "{stderr}");
    assert!(
        backed_up(&sandbox).contains_key(Path::new("claude-code/old-copy/SKILL.md")),
        "{stderr}"
    );
}
