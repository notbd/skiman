//! The content of the distributed `skiman-manual` skill.

use saphyr::LoadableYamlNode;

use crate::support::Sandbox;

fn materialised(sandbox: &Sandbox) -> String {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.canon())
        .assert()
        .code(0);
    std::fs::read_to_string(sandbox.canon().join(".skiman/skiman-manual/SKILL.md")).unwrap()
}

#[test]
fn materialised_skiman_manual_passes_every_skill_format_rule() {
    let sandbox = Sandbox::new();
    let content = materialised(&sandbox);
    let folder = sandbox.canon().join(".skiman/skiman-manual");

    // Frontmatter starts on line 1 and is valid YAML without duplicate keys.
    let rest = content
        .strip_prefix("---\n")
        .expect("frontmatter on line 1");
    let (frontmatter, _body) = rest.split_once("\n---\n").expect("closing ---");
    let docs = saphyr::Yaml::load_from_str(frontmatter).expect("valid YAML");
    let [yaml] = docs.as_slice() else {
        panic!("one YAML document")
    };
    let name = yaml["name"].as_str().expect("name");
    let description = yaml["description"].as_str().expect("description");
    assert_eq!(name, "skiman-manual", "name equals the folder name");
    assert!(description.starts_with("skiman's manual "), "{description}");
    assert!(content.is_ascii(), "the manual is plain ASCII English");
    let len = description.chars().count();
    assert!((1..=1024).contains(&len), "description is {len} characters");

    // One regular file, far below the size limits; no conflict markers.
    let entries: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].file_type().unwrap().is_file());
    assert!(content.len() < 10 * 1024 * 1024);
    for marker in ["<<<<<<< local (skiman)", ">>>>>>> upstream (skiman)"] {
        assert!(!content.lines().any(|l| l.starts_with(marker)));
    }
}

#[test]
fn skiman_manual_matches_its_snapshot_with_the_canon_path_filled_in() {
    let sandbox = Sandbox::new();
    let content = materialised(&sandbox);
    let repo = sandbox.canon().display().to_string();
    assert!(content.contains(&format!("The canon is `{repo}`.")));

    snapbox::assert_data_eq!(
        content.replace(&repo, "<CANON>"),
        snapbox::file!["snapshots/skiman-manual.md"].raw()
    );
}

/// Every everyday operation the ticket lists, with the text that tells an
/// agent how to do it the canonical way.
#[test]
fn skiman_manual_covers_every_everyday_operation() {
    let sandbox = Sandbox::new();
    let content = materialised(&sandbox);
    let repo = sandbox.canon().display().to_string();
    let required = [
        ("find the canon", format!("The canon is `{repo}`.")),
        ("create", "skiman new <name> -d".to_owned()),
        (
            "write the body",
            format!("{repo}/skills/<group>/<name>/SKILL.md"),
        ),
        (
            "install a named source",
            "skiman add <source> --yes".to_owned(),
        ),
        ("report the files", "which files came in".to_owned()),
        (
            "dry-run an unnamed source",
            "skiman add <source> --dry-run".to_owned(),
        ),
        (
            "wait for approval",
            "wait for the user's approval".to_owned(),
        ),
        ("edit in the canon", format!("{repo}/skills/<name>/")),
        ("update", "skiman update".to_owned()),
        ("resolve conflicts", "<<<<<<< local (skiman)".to_owned()),
        ("remove", "skiman remove <name> --yes".to_owned()),
        ("rename", "skiman mv <old> <new>".to_owned()),
        (
            "move to another group",
            "skiman mv <name> --group <group>".to_owned(),
        ),
        ("disable", "enabled = false".to_owned()),
        ("shared body", format!("{repo}/instructions/shared.md")),
        ("import", "skiman import --guide".to_owned()),
        (
            "adopt a foreign skill",
            "skiman adopt <name> --from <tool id> --yes".to_owned(),
        ),
        (
            "register a project root",
            "skiman project add <path>".to_owned(),
        ),
        ("tool directories", "~/.claude/skills".to_owned()),
        ("tool directories", "~/.agents/skills".to_owned()),
        ("tool directories", "~/.gemini/config/skills".to_owned()),
        (
            "no folders there",
            "Never create a folder in a tool directory".to_owned(),
        ),
        ("no edits of copies", "never edit what is there".to_owned()),
        ("no prompts", "never prompts".to_owned()),
        ("exit 2", "Exit code 2".to_owned()),
        // Linked skills (spec #37, user story 15).
        (
            "linked skills are outside",
            "A linked skill's folder is outside the canon".to_owned(),
        ),
        (
            "edit in the linked folder",
            "edit it in its linked folder".to_owned(),
        ),
        ("where that folder is", "`skiman list` shows".to_owned()),
        ("approval through linked/", format!("{repo}/linked/<id>/")),
        ("approval through linked/", "approval".to_owned()),
        (
            "search does not reach",
            "does not reach linked skills".to_owned(),
        ),
        ("register", "skiman linked add <path>".to_owned()),
        ("unregister", "skiman linked remove <id>".to_owned()),
        ("summary", "metadata.summary".to_owned()),
        (
            "refusals",
            "refuse a name that is only a linked skill".to_owned(),
        ),
    ];
    // Line breaks fall anywhere in the prose, so compare with single spaces.
    let flat = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let missing: Vec<_> = required
        .iter()
        .filter(|(_, text)| !flat.contains(text.as_str()))
        .collect();
    assert!(missing.is_empty(), "missing: {missing:#?}");
}

/// The manual's folder is a link: `skiman sync` cannot write the manual and
/// names the folder once (exit 4); doctor says the same line.
#[test]
fn a_manual_that_cannot_be_written_is_named_once() {
    let sandbox = Sandbox::new();
    materialised(&sandbox);
    let manual = sandbox.canon().join(".skiman/skiman-manual");
    std::fs::remove_dir_all(&manual).unwrap();
    let elsewhere = sandbox.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &manual).unwrap();
    let line = format!(
        "cannot write skiman-manual: {} is not a real folder (a link or a file is in the way)",
        manual.display()
    );

    let sync = sandbox.skiman().arg("sync").output().unwrap();
    let doctor = sandbox.skiman().arg("doctor").output().unwrap();

    assert_eq!(sync.status.code(), Some(4), "{sync:?}");
    let stderr = String::from_utf8(sync.stderr).unwrap();
    assert!(
        stderr.lines().any(|l| l == format!("error: {line}")),
        "{stderr}"
    );
    assert_eq!(doctor.status.code(), Some(4), "{doctor:?}");
    let stdout = String::from_utf8(doctor.stdout).unwrap();
    assert!(stdout.lines().any(|l| l == line), "{stdout}");
}
