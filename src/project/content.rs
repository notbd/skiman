//! Reading one side of a project item into an in-memory snapshot, and its
//! content hash. The same reader serves the project side and the mirror
//! side, so both hashes mean the same thing.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::pattern::Kind;
use crate::rules::LIMITS;
use crate::snapshot::{Snapshot, SnapshotFile};
use crate::tree::{files_hash, hex};

/// The content of one item side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    File(Vec<u8>),
    Skill(Vec<SnapshotFile>),
}

impl Content {
    /// `sha256:<hex>`: of the bytes for a file, the tree hash for a skill.
    pub fn hash(&self) -> String {
        match self {
            Content::File(bytes) => format!("sha256:{}", hex(&Sha256::digest(bytes))),
            Content::Skill(files) => files_hash(
                files
                    .iter()
                    .map(|f| (f.path.as_str(), f.executable, f.content.as_slice())),
            ),
        }
    }

    /// The files as they sit below the item's path: one file with an empty
    /// path for a file item.
    pub fn files(&self) -> Vec<(String, bool, &[u8])> {
        match self {
            Content::File(bytes) => vec![(String::new(), false, bytes.as_slice())],
            Content::Skill(files) => files
                .iter()
                .map(|f| (f.path.clone(), f.executable, f.content.as_slice()))
                .collect(),
        }
    }
}

/// What is at one side of an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    Absent,
    /// Something other than a regular file (file item) or a real folder (skill item).
    WrongKind,
    /// The right kind, but it cannot be taken as it is (a symlink inside a
    /// skill, a file over the size limit, no `SKILL.md`): why.
    Unsafe(String),
    Present(Content),
}

/// Reads `path` as an item of `kind`. Never follows a symlink at `path`.
pub fn read(path: &Path, kind: Kind) -> std::io::Result<Side> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Side::Absent),
        Err(e) => return Err(e),
    };
    match kind {
        Kind::File if meta.is_file() => {
            if meta.len() > LIMITS.file_bytes {
                return Ok(Side::Unsafe(format!(
                    "{} bytes is over the limit of {} bytes",
                    meta.len(),
                    LIMITS.file_bytes
                )));
            }
            Ok(Side::Present(Content::File(std::fs::read(path)?)))
        }
        Kind::Skill if meta.is_dir() => match Snapshot::read_local(path) {
            Err(failure) => Ok(Side::Unsafe(failure.message)),
            Ok(snapshot) if snapshot.file("SKILL.md").is_none() => Ok(Side::Unsafe(
                "no SKILL.md at the top of the skill folder".to_owned(),
            )),
            Ok(snapshot) => Ok(Side::Present(Content::Skill(snapshot.files))),
        },
        _ => Ok(Side::WrongKind),
    }
}

/// The hash of what is at `path`, when it is a readable item of `kind`.
pub fn hash_of(path: &Path, kind: Kind) -> std::io::Result<Option<String>> {
    Ok(match read(path, kind)? {
        Side::Present(content) => Some(content.hash()),
        _ => None,
    })
}
