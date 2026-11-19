//! Names and collisions (spec "Names and collisions"): tool directories are
//! flat, so a linked skill's name competes with every other skill's.

use super::{append_manifest, folder};
use crate::support::{Sandbox, Tool, init, skill_md, write};

/// Runs `skiman sync` and returns its exit code and stderr.
fn sync(sandbox: &Sandbox) -> (i32, String) {
    let output = sandbox.skiman().arg("sync").output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

fn target(sandbox: &Sandbox, name: &str) -> std::path::PathBuf {
    std::fs::read_link(sandbox.tool_dir(Tool::ClaudeCode).join(name)).unwrap()
}

/// Every skill folder under `skills/` holds its name, enabled or not, so
/// disabling it never hands the name to a linked skill.
#[test]
fn a_canon_skill_enabled_or_not_wins_over_a_linked_skill() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    for name in ["pdf", "docx"] {
        write(
            sandbox
                .skill_repo()
                .join("skills/office")
                .join(name)
                .join("SKILL.md"),
            skill_md(name),
        );
    }
    append_manifest(&sandbox, "\n[skills.docx]\nenabled = false\n");
    let company = folder(&sandbox, "work/company", &["pdf", "docx", "alpha"]);
    append_manifest(&sandbox, "\n[linked.company]\npath = \"~/work/company\"\n");

    let (code, stderr) = sync(&sandbox);

    assert_eq!(code, 3, "{stderr}");
    for (name, path) in [("pdf", "skills/office/pdf"), ("docx", "skills/office/docx")] {
        let line = format!(
            "warning: linked skill {name} in {} is not distributed: the skill folder {path} uses the name",
            company.join(name).display()
        );
        assert!(stderr.contains(&line), "{line}\n{stderr}");
    }
    assert_eq!(
        target(&sandbox, "pdf"),
        sandbox.skill_repo().join("skills/office/pdf")
    );
    assert!(!sandbox.tool_dir(Tool::ClaudeCode).join("docx").exists());
    assert!(
        sandbox
            .tool_dir(Tool::ClaudeCode)
            .join("alpha/SKILL.md")
            .is_file()
    );
}

/// Between linked folders the id that sorts first wins; when it goes away,
/// the next one takes over.
#[test]
fn between_linked_folders_the_first_id_wins_and_the_next_takes_over() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    folder(&sandbox, "work/acme", &["pdf"]);
    let zeta = folder(&sandbox, "work/zeta", &["pdf"]);
    append_manifest(
        &sandbox,
        "\n[linked.zeta]\npath = \"~/work/zeta\"\n\n[linked.acme]\npath = \"~/work/acme\"\n",
    );
    let linked = sandbox.skill_repo().join("linked");

    let (code, stderr) = sync(&sandbox);
    assert_eq!(code, 3, "{stderr}");
    let line = format!(
        "warning: linked skill pdf in {} is not distributed: linked folder `acme` sorts first and uses the name",
        zeta.join("pdf").display()
    );
    assert!(stderr.contains(&line), "{stderr}");
    assert_eq!(target(&sandbox, "pdf"), linked.join("acme/pdf"));

    std::fs::remove_dir_all(sandbox.home().join("work/acme/pdf")).unwrap();
    let (code, stderr) = sync(&sandbox);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(target(&sandbox, "pdf"), linked.join("zeta/pdf"));
}

/// A link an older script made for a linked skill resolves to the same
/// folder: deleting it is safe, and the report prints the command.
#[test]
fn an_occupied_name_whose_link_resolves_to_the_same_folder_says_it_is_safe_to_delete() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    let old = sandbox.tool_dir(Tool::ClaudeCode).join("alpha");
    std::os::unix::fs::symlink(company.join("alpha"), &old).unwrap();
    append_manifest(&sandbox, "\n[linked.company]\npath = \"~/work/company\"\n");

    let (code, stderr) = sync(&sandbox);

    assert_eq!(code, 3, "{stderr}");
    let line = format!(
        "warning: {} is a link to the same folder as skill alpha, but skiman did not make it; deleting it is safe: rm {}",
        old.display(),
        old.display()
    );
    assert!(stderr.contains(&line), "{line}\n{stderr}");
    std::fs::remove_file(&old).unwrap();
    assert_eq!(sync(&sandbox).0, 0);
    assert_eq!(
        target(&sandbox, "alpha"),
        sandbox.skill_repo().join("linked/company/alpha")
    );
}

/// An old link under another name that resolves into a registered folder
/// gets no `skiman adopt` hint, which would copy linked content into the
/// canon; a link into an unregistered folder outside the canon gets the
/// `skiman linked add` hint besides the adopt hint.
#[test]
fn a_foreign_link_into_a_registered_folder_gets_no_adopt_hint() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    let other = folder(&sandbox, "work/other", &["beta"]);
    let dir = sandbox.tool_dir(Tool::ClaudeCode);
    std::os::unix::fs::symlink(company.join("alpha"), dir.join("old-alpha")).unwrap();
    std::os::unix::fs::symlink(other.join("beta"), dir.join("beta")).unwrap();
    append_manifest(&sandbox, "\n[linked.company]\npath = \"~/work/company\"\n");

    let (code, stderr) = sync(&sandbox);

    assert_eq!(code, 0, "{stderr}");
    let old = dir.join("old-alpha");
    let line = format!(
        "warning: {} is a link into linked folder `company`, which skiman already distributes; delete it once nothing needs it",
        old.display()
    );
    assert!(stderr.contains(&line), "{line}\n{stderr}");
    assert!(!stderr.contains("adopt old-alpha"), "{stderr}");

    // Nor does `init`, with or without skills in the canon.
    std::fs::remove_file(dir.join("beta")).unwrap();
    let output = sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .output()
        .unwrap();
    let init_stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!init_stderr.contains("hint:"), "{init_stderr}");
    let line = format!(
        "warning: foreign skill {} is not managed by skiman; bring it in with `skiman adopt beta --from claude-code`, or register the folder it is in with `skiman linked add {}`",
        dir.join("beta").display(),
        other.display()
    );
    assert!(stderr.contains(&line), "{line}\n{stderr}");
}
