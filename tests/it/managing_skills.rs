//! The content of the distributed `managing-skills` skill.

use saphyr::LoadableYamlNode;

use crate::support::Sandbox;

fn materialised(sandbox: &Sandbox) -> String {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
    std::fs::read_to_string(
        sandbox
            .skill_repo()
            .join(".skiman/managing-skills/SKILL.md"),
    )
    .unwrap()
}

#[test]
fn materialised_managing_skills_passes_every_skill_format_rule() {
    let sandbox = Sandbox::new();
    let content = materialised(&sandbox);
    let folder = sandbox.skill_repo().join(".skiman/managing-skills");

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
    assert_eq!(name, "managing-skills", "name equals the folder name");
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
fn managing_skills_matches_its_snapshot_with_the_skill_repo_path_filled_in() {
    let sandbox = Sandbox::new();
    let content = materialised(&sandbox);
    let repo = sandbox.skill_repo().display().to_string();
    assert!(content.contains(&format!("The skill repo is `{repo}`.")));

    snapbox::assert_data_eq!(
        content.replace(&repo, "<SKILL_REPO>"),
        snapbox::file!["snapshots/managing-skills.md"].raw()
    );
}

/// Every everyday operation the ticket lists, with the text that tells an
/// agent how to do it the canonical way.
#[test]
fn managing_skills_covers_every_everyday_operation() {
    let sandbox = Sandbox::new();
    let content = materialised(&sandbox);
    let repo = sandbox.skill_repo().display().to_string();
    let required = [
        (
            "find the skill repo",
            format!("The skill repo is `{repo}`."),
        ),
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
        ("edit in the skill repo", format!("{repo}/skills/<name>/")),
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
    ];
    let missing: Vec<_> = required
        .iter()
        .filter(|(_, text)| !content.contains(text.as_str()))
        .collect();
    assert!(missing.is_empty(), "missing: {missing:#?}");
}
