//! `skiman list`: one row per skill with its kind, source, followed ref or
//! pinned version, local modifications, unresolved conflicts and whether it
//! is enabled. Read-only.

use std::path::Path;

use snapbox::str;

use crate::support::{Sandbox, init, read};

/// Replaces the first occurrence of `from` in the file at `path`.
fn edit(path: impl AsRef<Path>, from: &str, to: &str) {
    let text = read(&path);
    assert!(
        text.contains(from),
        "{from:?} not in {}",
        path.as_ref().display()
    );
    std::fs::write(path.as_ref(), text.replacen(from, to, 1)).unwrap();
}

/// Installs `acme/<name>`, a one-skill upstream, with `skiman add`.
fn add(sandbox: &Sandbox, name: &str) -> String {
    let up = sandbox.upstream(&format!("acme/{name}"));
    up.write(
        "SKILL.md",
        format!("---\nname: {name}\ndescription: d\n---\n\n# {name}\n"),
    );
    let commit = up.commit("v1");
    sandbox
        .skiman()
        .args(["add", &format!("acme/{name}"), "--yes"])
        .assert()
        .code(0);
    commit
}

#[test]
fn list_shows_an_own_skill() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes"])
        .assert()
        .code(0);

    sandbox
        .skiman()
        .arg("list")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
NAME   KIND  SOURCE  VERSION  MODIFIED  CONFLICTS  ENABLED
notes  own   -       -        -         no         yes

"#]])
        .stderr_eq("");
}

#[test]
fn list_shows_every_column_for_each_kind_of_skill() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes"])
        .assert()
        .code(0);
    let follows = add(&sandbox, "follows");
    let branch = add(&sandbox, "branch");
    let tag = add(&sandbox, "tag");
    let commit = add(&sandbox, "commit");
    let edited = add(&sandbox, "edited");
    let conflicted = add(&sandbox, "conflicted");
    let parked = add(&sandbox, "parked");
    let repo = sandbox.canon();

    // `add` has no --ref yet: pin and follow refs by hand, as the spec allows.
    let manifest = repo.join("skiman.toml");
    let lock = repo.join("skiman.lock");
    for (name, git_ref, kind) in [
        ("branch", "release/v2", "branch"),
        ("tag", "v1.0", "tag"),
        ("commit", commit.as_str(), "commit"),
    ] {
        let header = format!("[skills.{name}.source]\n");
        edit(
            &manifest,
            &header,
            &format!("{header}ref = \"{git_ref}\"\nref_kind = \"{kind}\"\n"),
        );
        let entry = format!("[skills.{name}]\n");
        edit(&lock, &entry, &format!("{entry}ref = \"{git_ref}\"\n"));
    }
    edit(
        repo.join("skills/edited/SKILL.md"),
        "# edited",
        "# edited locally",
    );
    std::fs::write(
        repo.join("skills/conflicted/notes.md"),
        "<<<<<<< local (skiman)\nmine\n=======\ntheirs\n>>>>>>> upstream (skiman)\n",
    )
    .unwrap();
    edit(
        &manifest,
        "[skills.parked.source]\n",
        "[skills.parked]\nenabled = false\n\n[skills.parked.source]\n",
    );

    let output = sandbox.skiman().arg("list").assert().code(0).stderr_eq("");
    let short = |sha: &str| sha[..7].to_owned();
    let expected = [
        [
            "NAME",
            "KIND",
            "SOURCE",
            "VERSION",
            "MODIFIED",
            "CONFLICTS",
            "ENABLED",
        ]
        .map(String::from),
        third_party(
            "branch",
            format!("follows branch release/v2 (at {})", short(&branch)),
            "no",
            "no",
            "yes",
        ),
        third_party(
            "commit",
            format!("pinned to commit {}", short(&commit)),
            "no",
            "no",
            "yes",
        ),
        third_party(
            "conflicted",
            format!("follows the default branch (at {})", short(&conflicted)),
            "yes",
            "yes",
            "yes",
        ),
        third_party(
            "edited",
            format!("follows the default branch (at {})", short(&edited)),
            "yes",
            "no",
            "yes",
        ),
        third_party(
            "follows",
            format!("follows the default branch (at {})", short(&follows)),
            "no",
            "no",
            "yes",
        ),
        ["notes", "own", "-", "-", "-", "no", "yes"].map(String::from),
        third_party(
            "parked",
            format!("follows the default branch (at {})", short(&parked)),
            "no",
            "no",
            "no",
        ),
        third_party(
            "tag",
            format!("pinned to tag v1.0 ({})", short(&tag)),
            "no",
            "no",
            "yes",
        ),
    ];
    assert_eq!(columns(&output.get_output().stdout), expected);
}

fn third_party(
    name: &str,
    version: String,
    modified: &str,
    conflicts: &str,
    enabled: &str,
) -> [String; 7] {
    [
        name.to_owned(),
        "third-party".to_owned(),
        format!("https://github.com/acme/{name}.git"),
        version,
        modified.to_owned(),
        conflicts.to_owned(),
        enabled.to_owned(),
    ]
}

/// The rows of `skiman list` output, split into cells at runs of two or more spaces.
fn columns(stdout: &[u8]) -> Vec<[String; 7]> {
    let text = String::from_utf8(stdout.to_vec()).unwrap();
    text.lines()
        .map(|line| {
            let cells: Vec<String> = line
                .split("  ")
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_owned)
                .collect();
            cells
                .try_into()
                .unwrap_or_else(|c| panic!("not 7 columns: {c:?}"))
        })
        .collect()
}

#[test]
fn list_shows_the_upstream_folder_of_a_skill_inside_a_bigger_repo() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    // Multi-skill sources need --skill (another ticket): install a
    // single-skill upstream, then point its recorded path at a sub-folder.
    let single = sandbox.upstream("acme/pdf");
    single.write("SKILL.md", "---\nname: pdf\ndescription: d\n---\n");
    single.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);
    edit(
        repo.join("skiman.toml"),
        "path = \"\"",
        "path = \"skills/pdf\"",
    );

    sandbox
        .skiman()
        .arg("list")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
NAME  KIND         SOURCE                                        VERSION                                  MODIFIED  CONFLICTS  ENABLED
pdf   third-party  https://github.com/acme/pdf.git (skills/pdf)  follows the default branch (at [..])  no        no         yes

"#]]);
}
