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
    let (repo, agents) = (
        path(&sandbox.skill_repo()),
        path(&company.join("AGENTS.md")),
    );
    let (company_path, work_path) = (path(&company), path(&work));
    // Each command and the exit it must end with, so none passes by doing nothing.
    let steps: [(&[&str], i32); 17] = [
        (&["linked", "add", &company_path], 0),
        (&["linked", "add", &company_path], 0),
        (&["sync"], 0),
        (&["sync", "--hook"], 0),
        (&["check"], 0),
        (&["doctor"], 0),
        (&["list"], 0),
        (&["overview"], 0),
        (&["new", "notes", "-d", "Takes notes"], 0),
        (&["project", "add", &work_path], 0),
        (&["project", "adopt", "--yes"], 0),
        // Named exactly, the item that overlaps the folder is skipped.
        (&["project", "adopt", "--yes", &agents], 1),
        (&["project", "restore", "--yes", &work_path], 0),
        (&["init", "--session-hooks", "claude-code", &repo], 0),
        (&["init", "--session-hooks", "none", &repo], 0),
        (&["remove", "notes", "--yes"], 0),
        (&["linked", "remove", "company"], 0),
    ];
    for (args, code) in steps {
        let output = sandbox.skiman().args(args).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "skiman {args:?}: {output:?}"
        );
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
