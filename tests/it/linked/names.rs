//! Names and collisions (spec "Names and collisions"): tool directories are
//! flat, so a linked skill's name competes with every other skill's.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{COMPANY, append_manifest, folder, sync};
use crate::support::{Sandbox, Tool, init, skill_md, tree, write};

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
                .canon()
                .join("skills/office")
                .join(name)
                .join("SKILL.md"),
            skill_md(name),
        );
    }
    append_manifest(&sandbox, "\n[skills.docx]\nenabled = false\n");
    let company = folder(&sandbox, "work/company", &["pdf", "docx", "alpha"]);
    append_manifest(&sandbox, COMPANY);

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
        sandbox.canon().join("skills/office/pdf")
    );
    assert!(!sandbox.tool_dir(Tool::ClaudeCode).join("docx").exists());
    assert!(
        sandbox
            .tool_dir(Tool::ClaudeCode)
            .join("alpha/SKILL.md")
            .is_file()
    );
}

/// A group folder's name is taken like a skill's, at any depth (ADR 0009),
/// so a linked skill of that name is not distributed.
#[test]
fn a_group_folder_wins_over_a_linked_skill() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(
        sandbox
            .canon()
            .join("skills/third-party/tstack/tool/SKILL.md"),
        skill_md("tool"),
    );
    let company = folder(
        &sandbox,
        "work/company",
        &["third-party", "tstack", "alpha"],
    );
    append_manifest(&sandbox, COMPANY);

    let (code, stderr) = sync(&sandbox);

    assert_eq!(code, 3, "{stderr}");
    for (name, path) in [
        ("third-party", "skills/third-party"),
        ("tstack", "skills/third-party/tstack"),
    ] {
        let line = format!(
            "warning: linked skill {name} in {} is not distributed: the group folder {path} uses the name",
            company.join(name).display()
        );
        assert!(stderr.contains(&line), "{line}\n{stderr}");
        assert!(!sandbox.tool_dir(Tool::ClaudeCode).join(name).exists());
    }
    for name in ["tool", "alpha"] {
        let skill = sandbox.tool_dir(Tool::ClaudeCode).join(name);
        assert!(skill.join("SKILL.md").is_file(), "{name}");
    }
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
    let linked = sandbox.canon().join("linked");

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

/// A linked skill whose `SKILL.md` cannot be probed holds its name only
/// through the tool-directory links that already point at it: a readable
/// winner keeps its link while a loser cannot be probed, a readable loser
/// does not take over from a winner that cannot be probed, and a new entry
/// that cannot be probed takes no name from the current winner. `list`
/// shows the winner the tool directories hold.
#[test]
fn a_skill_that_cannot_be_probed_changes_no_collision_winner() {
    use std::os::unix::fs::PermissionsExt;
    let chmod = |path: &std::path::Path, mode| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    // Which folder's `pdf` cannot be probed, and whether it is new.
    for (locked, new) in [("zeta", false), ("acme", false), ("acme", true)] {
        let case = format!("{locked}{}", if new { " (new)" } else { "" });
        let sandbox = Sandbox::new();
        init(&sandbox);
        folder(
            &sandbox,
            "work/acme",
            if new { &["other"] } else { &["pdf"] },
        );
        folder(&sandbox, "work/zeta", &["pdf"]);
        append_manifest(
            &sandbox,
            "\n[linked.zeta]\npath = \"~/work/zeta\"\n\n[linked.acme]\npath = \"~/work/acme\"\n",
        );
        let linked = sandbox.canon().join("linked");
        sync(&sandbox);
        let winner = linked.join(if new { "zeta/pdf" } else { "acme/pdf" });
        assert_eq!(target(&sandbox, "pdf"), winner, "{case}");
        let skill = sandbox.home().join("work").join(locked).join("pdf");
        if new {
            write(skill.join("SKILL.md"), skill_md("pdf"));
        }

        chmod(&skill, 0o000);
        let (code, stderr) = sync(&sandbox);
        let list = sandbox.skiman().arg("list").output().unwrap().stdout;
        let overview = sandbox.skiman().arg("overview").output().unwrap().stdout;
        chmod(&skill, 0o755);

        assert_eq!(code, 3, "{case}: {stderr}");
        let unchecked = format!("linked skill pdf of `{locked}` is not checked");
        assert!(stderr.contains(&unchecked), "{case}: {stderr}");
        assert!(!stderr.contains("removed"), "{case}: {stderr}");
        for tool in Tool::ALL {
            let link = sandbox.tool_dir(tool).join("pdf");
            assert_eq!(std::fs::read_link(link).unwrap(), winner, "{case}");
        }
        let acme = sandbox.home().join("work/acme/pdf");
        let zeta = sandbox.home().join("work/zeta/pdf");
        let lost = |dir: &std::path::Path| {
            format!("linked skill pdf in {} is not distributed", dir.display())
        };
        assert!(!stderr.contains(&lost(&acme)), "{case}: {stderr}");
        let list = String::from_utf8(list).unwrap();
        let overview = String::from_utf8(overview).unwrap();
        let kept = "linked skill pdf of `acme`, whose SKILL.md cannot be read now, keeps the name";
        if locked == "acme" && !new {
            assert!(
                stderr.contains(&format!("{}: {kept}", lost(&zeta))),
                "{stderr}"
            );
            assert!(list.contains(&format!("no: {kept}")), "{list}");
            assert!(overview.contains(&format!("no: {kept}")), "{overview}");
        } else {
            assert!(!stderr.contains(&lost(&zeta)), "{case}: {stderr}");
            assert!(!list.contains("cannot be read now"), "{case}: {list}");
            assert!(
                !overview.contains("cannot be read now"),
                "{case}: {overview}"
            );
        }
    }
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
    append_manifest(&sandbox, COMPANY);

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
        sandbox.canon().join("linked/company/alpha")
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
    append_manifest(&sandbox, COMPANY);

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
        .arg(sandbox.canon())
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

/// The canon, every tool directory and the linked folder `company`: a
/// refused command leaves all of them as they were.
fn world(sandbox: &Sandbox, company: &std::path::Path) -> Vec<BTreeMap<PathBuf, String>> {
    let mut world = vec![tree(sandbox.canon()), tree(company)];
    world.extend(Tool::ALL.iter().map(|t| tree(sandbox.tool_dir(*t))));
    world
}

/// Runs skiman with `args` and returns its exit code and stderr.
fn run(sandbox: &Sandbox, args: &[&str]) -> (i32, String) {
    let output = sandbox.skiman().args(args).output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// Registers the linked folder `company` (skills `alpha`, `pdf`) in an
/// initialised canon and syncs; returns the folder and sync's exit code.
fn register(sandbox: &Sandbox) -> (PathBuf, i32) {
    let company = folder(sandbox, "work/company", &["alpha", "pdf"]);
    append_manifest(sandbox, COMPANY);
    (company, sync(sandbox).0)
}

/// A canon with the linked folder `company` registered and synced.
fn registered(sandbox: &Sandbox) -> PathBuf {
    init(sandbox);
    let (company, code) = register(sandbox);
    assert_eq!(code, 0);
    company
}

/// The commands that act on an existing skill refuse a name that is only
/// a linked skill, naming the folder it is edited in; nothing changes.
#[test]
fn commands_that_act_on_a_skill_refuse_a_name_that_is_only_a_linked_skill() {
    let sandbox = Sandbox::new();
    let company = registered(&sandbox);
    write(
        sandbox.canon().join("skills/beta/SKILL.md"),
        skill_md("beta"),
    );
    assert_eq!(sync(&sandbox).0, 0);
    let before = world(&sandbox, &company);
    let named = format!(
        "alpha is a linked skill in {}, in linked folder `company`; it is edited there",
        company.join("alpha").display()
    );

    for args in [
        &["update", "alpha"][..],
        &["diff", "alpha"],
        &["remove", "alpha", "--yes"],
        &["mv", "alpha", "beta"],
        &["mv", "alpha", "gamma"],
        &["mv", "alpha", "--group", "office"],
    ] {
        let (code, stderr) = run(&sandbox, args);
        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(stderr.contains(&named), "{args:?}: {stderr}");
        assert_eq!(world(&sandbox, &company), before, "{args:?}");
    }
    let (_, stderr) = run(&sandbox, &["remove", "alpha", "--yes"]);
    assert!(
        stderr.contains("run `skiman linked remove company`"),
        "{stderr}"
    );
}

/// A canon skill that shares its name with a linked skill holds the name,
/// so the commands act on it: the collision can be repaired with them.
#[test]
fn commands_that_act_on_a_skill_work_on_a_canon_skill_that_collides_with_a_linked_one() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", skill_md("pdf"));
    up.commit("v1");
    assert_eq!(run(&sandbox, &["add", "acme/pdf", "--yes"]).0, 0);
    assert_eq!(register(&sandbox).1, 3, "the linked `pdf` loses the name");
    let linked = sandbox.canon().join("linked");

    let (code, stderr) = run(&sandbox, &["diff", "pdf"]);
    assert_eq!(code, 0, "{stderr}");
    // Each closing sync reports the linked `pdf` that is not distributed.
    let (code, stderr) = run(&sandbox, &["update", "pdf"]);
    assert_eq!(code, 3, "{stderr}");
    assert!(stderr.contains("linked skill pdf in "), "{stderr}");
    // A move to another group under the same name gives no new name.
    let (code, stderr) = run(&sandbox, &["mv", "pdf", "--group", "office"]);
    assert_eq!(code, 3, "{stderr}");
    assert!(sandbox.canon().join("skills/office/pdf/SKILL.md").is_file());
    assert_eq!(
        target(&sandbox, "pdf"),
        sandbox.canon().join("skills/office/pdf")
    );

    let (code, stderr) = run(&sandbox, &["remove", "pdf", "--yes"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!sandbox.canon().join("skills/office/pdf").exists());
    assert_eq!(target(&sandbox, "pdf"), linked.join("company/pdf"));

    // A leftover record under a linked skill's name is still the canon's to clean up.
    append_manifest(&sandbox, "\n[skills.alpha]\nsummary = \"Alpha\"\n");
    let (code, stderr) = run(&sandbox, &["remove", "alpha", "--yes"]);
    assert_eq!(code, 0, "{stderr}");
    let manifest = std::fs::read_to_string(sandbox.canon().join("skiman.toml")).unwrap();
    assert!(!manifest.contains("[skills.alpha]"), "{manifest}");
    assert_eq!(target(&sandbox, "alpha"), linked.join("company/alpha"));
}

/// The commands that create a name refuse a linked skill's name, whether
/// it is distributed or not, and nothing changes; a free name works.
#[test]
fn commands_that_create_a_name_refuse_a_linked_skills_name() {
    let sandbox = Sandbox::new();
    let company = registered(&sandbox);
    write(
        sandbox.canon().join("skills/beta/SKILL.md"),
        skill_md("beta"),
    );
    assert_eq!(sync(&sandbox).0, 0);
    let up = sandbox.upstream("acme/alpha");
    up.write("SKILL.md", skill_md("alpha"));
    up.commit("v1");
    let taken = format!(
        "the name `alpha` is taken: it is a linked skill in {}, in linked folder `company`",
        company.join("alpha").display()
    );
    let before = world(&sandbox, &company);
    for args in [
        &["new", "alpha", "-d", "Does alpha things"][..],
        &["add", "acme/alpha", "--yes"],
        &["mv", "beta", "alpha"],
    ] {
        let (code, stderr) = run(&sandbox, args);
        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(stderr.contains(&taken), "{args:?}: {stderr}");
        assert_eq!(world(&sandbox, &company), before, "{args:?}");
    }
    // A foreign folder under the linked skill's name in one tool directory.
    let antigravity = sandbox.tool_dir(Tool::Antigravity);
    std::fs::remove_file(antigravity.join("alpha")).unwrap();
    write(antigravity.join("alpha/SKILL.md"), skill_md("alpha"));
    let before = world(&sandbox, &company);
    let (code, stderr) = run(
        &sandbox,
        &["adopt", "alpha", "--from", "antigravity", "--yes"],
    );
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains(&taken), "{stderr}");
    assert_eq!(world(&sandbox, &company), before);

    std::fs::rename(antigravity.join("alpha"), antigravity.join("delta")).unwrap();
    for args in [
        &["adopt", "delta", "--from", "antigravity", "--yes"][..],
        &["new", "gamma", "-d", "Does gamma things"],
        &["add", "acme/alpha", "--yes", "--name", "alpha-two"],
        &["mv", "beta", "beta-two"],
    ] {
        let (code, stderr) = run(&sandbox, args);
        assert_eq!(code, 0, "{args:?}: {stderr}");
    }
    let skills = sandbox.canon().join("skills");
    for name in ["delta", "gamma", "alpha-two", "beta-two"] {
        assert!(skills.join(name).join("SKILL.md").is_file(), "{name}");
    }
    assert_eq!(
        target(&sandbox, "alpha"),
        sandbox.canon().join("linked/company/alpha")
    );
}

/// A group folder's name is taken like a skill's (ADR 0009), so every
/// command that creates a group folder refuses a linked skill's name, at
/// any depth, and nothing changes; the linked skill stays distributed.
#[test]
fn commands_that_create_a_group_folder_refuse_a_linked_skills_name() {
    let sandbox = Sandbox::new();
    let company = registered(&sandbox);
    write(
        sandbox.canon().join("skills/beta/SKILL.md"),
        skill_md("beta"),
    );
    write(
        sandbox.tool_dir(Tool::Antigravity).join("delta/SKILL.md"),
        skill_md("delta"),
    );
    let up = sandbox.upstream("acme/tool");
    up.write("SKILL.md", skill_md("tool"));
    up.commit("v1");
    assert_eq!(sync(&sandbox).0, 0);
    let before = world(&sandbox, &company);
    for (group, args) in [
        ("alpha", &["new", "foo", "-d", "Does foo things"][..]),
        ("office/alpha", &["new", "foo", "-d", "Does foo things"]),
        ("alpha", &["add", "acme/tool", "--yes"]),
        (
            "alpha",
            &["adopt", "delta", "--from", "antigravity", "--yes"],
        ),
        ("alpha", &["mv", "beta"]),
    ] {
        let (code, stderr) = run(&sandbox, &[args, &["--group", group]].concat());
        assert_eq!(code, 1, "{args:?} {group}: {stderr}");
        let line = format!(
            "`{group}` cannot be a group: `alpha` is a linked skill in {}, in linked folder `company`",
            company.join("alpha").display()
        );
        assert!(stderr.contains(&line), "{line}\n{stderr}");
        assert_eq!(world(&sandbox, &company), before, "{args:?} {group}");
    }
    assert_eq!(
        target(&sandbox, "alpha"),
        sandbox.canon().join("linked/company/alpha")
    );
}

/// A single-skill folder whose top-level `SKILL.md` cannot be probed, and
/// whose `<id>` link the tool directories still hold, stays one skill for
/// that sync: its sub-folder `kid` takes no part in naming, so the winner
/// of `kid` in another folder keeps its link.
#[test]
fn a_single_skill_that_cannot_be_probed_lends_no_name_to_its_sub_folders() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    init(&sandbox);
    let work = sandbox.home().join("work");
    folder(&sandbox, "work", &["a"]);
    write(work.join("a/kid/SKILL.md"), skill_md("kid"));
    folder(&sandbox, "work/b", &["kid"]);
    append_manifest(
        &sandbox,
        "\n[linked.a]\npath = \"~/work/a\"\n\n[linked.b]\npath = \"~/work/b\"\n",
    );
    let linked = sandbox.canon().join("linked");
    assert_eq!(sync(&sandbox).0, 0);
    assert_eq!(target(&sandbox, "kid"), linked.join("b/kid"));
    let locked = work.join("locked");
    write(locked.join("SKILL.md"), skill_md("a"));
    std::fs::remove_file(work.join("a/SKILL.md")).unwrap();
    std::os::unix::fs::symlink(locked.join("SKILL.md"), work.join("a/SKILL.md")).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let (code, stderr) = sync(&sandbox);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(code, 3, "{stderr}");
    assert!(
        stderr.contains("linked skill a of `a` is not checked"),
        "{stderr}"
    );
    assert!(!stderr.contains("sorts first"), "{stderr}");
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert_eq!(std::fs::read_link(dir.join("a")).unwrap(), linked.join("a"));
        assert_eq!(
            std::fs::read_link(dir.join("kid")).unwrap(),
            linked.join("b/kid")
        );
    }
}
