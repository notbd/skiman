//! The canon's `linked/` folder (spec "Sync"): it holds only the links
//! sync makes, one per linked folder; every link in it is skiman's and
//! nothing else in it is.

use std::os::unix::fs::symlink;

use super::{COMPANY, append_manifest, folder, sync};
use crate::support::{Sandbox, Tool, assert_each_kill_finishes, exists, init, read};

fn company() -> Sandbox {
    let sandbox = Sandbox::new();
    init(&sandbox);
    folder(&sandbox, "work/company", &["alpha"]);
    append_manifest(&sandbox, COMPANY);
    sandbox
}

/// `linked` that is a link or a file is reported, and no linked skill is
/// linked through it (exit 3); the canon's own skills sync as usual.
#[test]
fn linked_as_a_link_or_a_file_links_no_linked_skill() {
    for make in ["link", "file"] {
        let sandbox = company();
        let linked = sandbox.canon().join("linked");
        let elsewhere = sandbox.root().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        match make {
            "link" => symlink(&elsewhere, &linked).unwrap(),
            _ => std::fs::write(&linked, "x").unwrap(),
        }

        let (code, stderr) = sync(&sandbox);

        assert_eq!(code, 3, "{stderr}");
        let line = format!(
            "warning: {} is a {make}, not a folder, so no linked skill is linked",
            linked.display()
        );
        assert!(stderr.contains(&line), "{line}\n{stderr}");
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
        for tool in Tool::ALL {
            assert!(!exists(sandbox.tool_dir(tool).join("alpha")));
            assert!(exists(sandbox.tool_dir(tool).join("skiman-manual")));
        }
    }
}

/// A folder at `linked/<id>` occupies the id: it is reported and kept, and
/// the folder's skills are not offered (exit 3). Any other entry under
/// `linked/` that is not a link is reported and kept too; Finder's files
/// are not.
#[test]
fn entries_under_linked_that_are_not_links_are_reported_and_kept() {
    let sandbox = company();
    let linked = sandbox.canon().join("linked");
    std::fs::create_dir_all(linked.join("company")).unwrap();
    std::fs::write(linked.join("notes.txt"), "mine").unwrap();
    std::fs::write(linked.join(".DS_Store"), "").unwrap();

    let (code, stderr) = sync(&sandbox);

    assert_eq!(code, 3, "{stderr}");
    let occupied = format!(
        "warning: {} is not a link, so linked folder `company` is not linked",
        linked.join("company").display()
    );
    let stray = format!(
        "warning: {} is not a link skiman made, so skiman leaves it alone",
        linked.join("notes.txt").display()
    );
    assert!(
        stderr.contains(&occupied) && stderr.contains(&stray),
        "{stderr}"
    );
    assert!(!stderr.contains(".DS_Store"), "{stderr}");
    assert!(linked.join("company").is_dir());
    assert_eq!(read(linked.join("notes.txt")), "mine");
    for tool in Tool::ALL {
        assert!(!exists(sandbox.tool_dir(tool).join("alpha")));
    }
}

/// A link under `linked/` for an id that is no longer registered goes; so
/// does one left under a temporary name by a sync killed while it
/// retargeted `linked/<id>`, which the next sync finishes.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_sync_killed_while_it_retargets_linked_id_finishes_when_run_again() {
    assert_each_kill_finishes(
        |sandbox| {
            init(sandbox);
            folder(sandbox, "work/company", &["alpha"]);
            folder(sandbox, "moved/company", &["alpha"]);
            append_manifest(sandbox, COMPANY);
            sandbox.skiman().arg("sync").output().unwrap();
            let manifest = sandbox.canon().join("skiman.toml");
            let text = read(&manifest).replace("~/work/company", "~/moved/company");
            std::fs::write(&manifest, text).unwrap();
            std::os::unix::fs::symlink("/nowhere", sandbox.canon().join("linked/gone")).unwrap();
            vec!["sync".to_owned()]
        },
        &[
            "link:aside:<root>/canon/linked/company",
            "link:aside:<root>/canon/linked/gone",
        ],
    );
}
