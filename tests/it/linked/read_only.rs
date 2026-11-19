//! skiman never writes inside a linked folder (ADR 0010): every command
//! that touches linked folders, the project scope or the tool directories
//! leaves the folder and its child links' targets byte for byte as they were.

use std::os::unix::fs::symlink;

use super::contents;
use crate::support::{Sandbox, init, skill_md, write};

#[test]
fn no_command_writes_inside_a_linked_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let work = sandbox.home().join("work");
    let company = work.join("company");
    let independent = sandbox.root().join("independent");
    write(company.join("alpha/SKILL.md"), skill_md("alpha"));
    write(company.join("broken/SKILL.md"), "---\nname: [\n---\n");
    write(company.join("AGENTS.md"), "# company\n");
    write(company.join(".hidden/SKILL.md"), skill_md("hidden"));
    write(company.join("notes.txt"), "notes\n");
    write(independent.join("SKILL.md"), skill_md("tool"));
    symlink(&independent, company.join("tool")).unwrap();
    symlink(work.join("gone"), company.join("gone")).unwrap();
    write(work.join("AGENTS.md"), "# work\n");
    let before = contents(&[&company, &independent]);
    let path = |p: &std::path::Path| p.display().to_string();
    let steps: Vec<Vec<String>> = vec![
        vec!["linked".into(), "add".into(), path(&company)],
        vec!["linked".into(), "add".into(), path(&company)],
        vec!["sync".into()],
        vec!["sync".into(), "--hook".into()],
        vec!["check".into()],
        vec!["doctor".into()],
        vec!["list".into()],
        vec!["overview".into()],
        vec![
            "new".into(),
            "notes".into(),
            "-d".into(),
            "Takes notes".into(),
        ],
        vec!["project".into(), "add".into(), path(&work)],
        vec!["project".into(), "adopt".into(), "--yes".into()],
        vec![
            "project".into(),
            "adopt".into(),
            "--yes".into(),
            path(&company.join("AGENTS.md")),
        ],
        vec![
            "project".into(),
            "restore".into(),
            "--yes".into(),
            path(&work),
        ],
        vec![
            "init".into(),
            "--session-hooks".into(),
            "claude-code".into(),
            path(&sandbox.skill_repo()),
        ],
        vec![
            "init".into(),
            "--session-hooks".into(),
            "none".into(),
            path(&sandbox.skill_repo()),
        ],
        vec!["remove".into(), "notes".into(), "--yes".into()],
        vec!["linked".into(), "remove".into(), "company".into()],
    ];
    for args in steps {
        let output = sandbox.skiman().args(&args).output().unwrap();
        assert!(output.status.code().is_some(), "{args:?}: {output:?}");
        assert_eq!(
            contents(&[&company, &independent]),
            before,
            "after skiman {args:?}"
        );
    }
    assert!(
        sandbox
            .skill_repo()
            .join("projects/work/AGENTS.md")
            .is_file()
    );
    assert!(!sandbox.skill_repo().join("projects/work/company").exists());
}
