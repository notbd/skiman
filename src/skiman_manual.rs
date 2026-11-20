//! skiman's manual, the `skiman-manual` skill shipped in the binary. Sync
//! materialises it, with the canon path filled in, into
//! `<repo>/.skiman/skiman-manual/`.

use std::io;

use crate::canon::Canon;
use crate::fsutil;
use crate::tools::TOOLS;

pub const NAME: &str = "skiman-manual";

const TEMPLATE: &str = include_str!("../assets/skiman-manual/SKILL.md");
const CANON_PLACEHOLDER: &str = "{{CANON}}";

/// The `SKILL.md` content for this canon.
pub fn content(repo: &Canon) -> String {
    let home_paths = |path: fn(&crate::tools::AgentTool) -> &'static str| {
        TOOLS
            .iter()
            .map(|tool| format!("`~/{}`", path(tool)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    TEMPLATE
        .replace(CANON_PLACEHOLDER, &repo.root().display().to_string())
        .replace("{{TOOL_DIRECTORIES}}", &home_paths(|t| t.tool_dir))
        .replace("{{INSTRUCTION_FILES}}", &home_paths(|t| t.instruction_file))
        .replace(
            "{{PROJECT_INSTRUCTION_FILES}}",
            &code_list(&crate::project::pattern::instruction_patterns()),
        )
        .replace(
            "{{PROJECT_SKILL_DIRS}}",
            &code_list(&crate::project::pattern::skill_dir_patterns()),
        )
}

/// `a`, `b` and `c` written as inline code, joined with commas.
fn code_list(items: &[&str]) -> String {
    items
        .iter()
        .map(|i| format!("`{i}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `.skiman/skiman-manual/SKILL.md` is to be written (it is missing
/// or its content differs), read-only. Fails, naming the path, when a
/// folder on the way is not a real folder (a link there could lead the
/// write out of the repo) or `SKILL.md` itself is a folder; a link to a
/// folder is replaced, as any other entry there. Its one read is the one
/// `materialise` makes anyway, so a sync whose manual is current reads
/// nothing more; it never waits for a writer (a FIFO there reads as empty,
/// so it is replaced), and reads no more than one byte past the manual
/// (`/dev/zero` ends too).
pub fn needs_write(repo: &Canon) -> io::Result<bool> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = repo.manual_dir().join("SKILL.md");
    if let Some(why) = fsutil::broken_chain(repo.root(), &file) {
        return Err(io::Error::other(why));
    }
    let (wanted, mut current) = (content(repo), Vec::new());
    let flags = rustix::fs::OFlags::NONBLOCK.bits() as i32;
    let read = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(&file);
    let most = wanted.len() as u64 + 1;
    match read.and_then(|f| f.take(most).read_to_end(&mut current)) {
        Ok(_) => Ok(current != wanted.as_bytes()),
        // Only `SKILL.md` itself as a folder blocks the write.
        Err(e)
            if e.kind() == io::ErrorKind::IsADirectory
                && !std::fs::symlink_metadata(&file).is_ok_and(|m| m.is_symlink()) =>
        {
            Err(io::Error::other(format!("{} is a folder", file.display())))
        }
        Err(_) => Ok(true),
    }
}

/// Writes `.skiman/skiman-manual/SKILL.md`; sync calls it when
/// [`needs_write`] says so. An error names the path, as those of
/// `needs_write` do.
pub fn materialise(repo: &Canon) -> io::Result<()> {
    let file = repo.manual_dir().join("SKILL.md");
    std::fs::create_dir_all(repo.manual_dir())
        .and_then(|()| fsutil::write_derived(&file, content(repo).as_bytes()))
        .map_err(|e| io::Error::other(format!("{}: {e}", file.display())))
}
