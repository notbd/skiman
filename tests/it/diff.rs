//! `skiman diff <name>`: the local modifications of a third-party skill
//! against its upstream snapshot, as a unified diff. Read-only.

use std::os::unix::fs::PermissionsExt;

use snapbox::{IntoData, str};

use crate::support::{PDF_SKILL_MD, Sandbox, init, write};

/// Installs `acme/pdf` (`SKILL.md`, `reference.md`, `fill.sh`), with extra `add` args.
fn add_pdf(sandbox: &Sandbox, extra: &[&str]) {
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", PDF_SKILL_MD)
        .write("reference.md", "# Reference\n")
        .write_executable("fill.sh", "#!/bin/sh\necho fill\n");
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .args(extra)
        .assert()
        .code(0);
}

#[test]
fn diff_after_a_local_edit_shows_exactly_that_edit() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox, &[]);
    let skill = sandbox.skill_repo().join("skills/pdf");
    write(
        skill.join("SKILL.md"),
        PDF_SKILL_MD.replace("# pdf\n", "# pdf, edited\n"),
    );

    sandbox
        .skiman()
        .args(["diff", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
--- upstream/pdf/SKILL.md
+++ skills/pdf/SKILL.md
@@ -3,4 +3,4 @@
 description: Fill PDF forms
 ---
 
-# pdf
+# pdf, edited

"#]])
        .stderr_eq("");
}

#[test]
fn diff_of_an_unmodified_skill_prints_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox, &[]);

    sandbox
        .skiman()
        .args(["diff", "pdf"])
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
}

#[test]
fn diff_shows_the_name_line_add_rewrote_as_a_local_modification() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox, &["--name", "forms"]);

    sandbox
        .skiman()
        .args(["diff", "forms"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
--- upstream/forms/SKILL.md
+++ skills/forms/SKILL.md
@@ -1,5 +1,5 @@
 ---
-name: pdf
+name: forms
 description: Fill PDF forms
 ---
 

"#]])
        .stderr_eq("");
}

#[test]
fn diff_shows_added_deleted_binary_and_mode_changes() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox, &[]);
    let skill = sandbox.skill_repo().join("skills/pdf");
    std::fs::remove_file(skill.join("reference.md")).unwrap();
    std::fs::create_dir_all(skill.join("notes")).unwrap();
    write(skill.join("notes/todo.md"), "one\ntwo");
    std::fs::write(skill.join("logo.png"), b"\x89PNG\r\n\x1a\n\x00\x00").unwrap();
    std::fs::set_permissions(
        skill.join("fill.sh"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    // Ignored by diffing, like by the tree hash.
    write(skill.join(".DS_Store"), "finder");

    sandbox
        .skiman()
        .args(["diff", "pdf"])
        .assert()
        .code(0)
        // Raw: the default filters would turn `\ No newline` into `/ No newline`.
        .stdout_eq(
            str![[r#"
mode of skills/pdf/fill.sh changed from 100755 to 100644
Binary files /dev/null and skills/pdf/logo.png differ
--- /dev/null
+++ skills/pdf/notes/todo.md
@@ -0,0 +1,2 @@
+one
+two
\ No newline at end of file
--- upstream/pdf/reference.md
+++ /dev/null
@@ -1 +0,0 @@
-# Reference

"#]]
            .raw(),
        )
        .stderr_eq("");
}

#[test]
fn diff_refuses_an_own_skill_and_an_unknown_name() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes"])
        .assert()
        .code(0);

    sandbox
        .skiman()
        .args(["diff", "notes"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: notes is an own skill: it has no upstream snapshot to compare with

"#]]);
    sandbox
        .skiman()
        .args(["diff", "nothing"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: there is no skill folder skills/nothing

"#]]);
}
