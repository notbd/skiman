//! Hashes in the form skiman records them, computed here without skiman's
//! code: a test that compares skiman's value with one of these catches a
//! change on either side. Do not replace them with calls into `skiman::tree`.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `sha256:<hex>` of a secret's matched text, the fingerprint skiman reports
/// instead of the text.
pub fn fingerprint(matched: &str) -> String {
    format!("sha256:{}", hex(&Sha256::digest(matched.as_bytes())))
}

/// skiman's tree hash of the folder `dir`: regular files only, sorted by
/// path bytes, each adding `path \0 x \0 sha256 \n`; `.DS_Store`,
/// `Thumbs.db` and `Icon\r` are ignored.
pub fn tree_hash(dir: &Path) -> String {
    fn files(root: &Path, dir: &Path, out: &mut Vec<(String, bool, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                files(root, &path, out);
            } else if meta.is_file() {
                let name = path.file_name().unwrap().to_str().unwrap();
                if ![".DS_Store", "Thumbs.db", "Icon\r"].contains(&name) {
                    let rel = path.strip_prefix(root).unwrap().to_str().unwrap();
                    let exec = meta.permissions().mode() & 0o100 != 0;
                    out.push((rel.to_owned(), exec, std::fs::read(&path).unwrap()));
                }
            }
        }
    }
    let mut list = Vec::new();
    files(dir, dir, &mut list);
    list.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut text = Vec::new();
    for (rel, exec, content) in list {
        text.extend_from_slice(rel.as_bytes());
        text.extend_from_slice(if exec { b"\x001\x00" } else { b"\x000\x00" });
        text.extend_from_slice(hex(&Sha256::digest(&content)).as_bytes());
        text.push(b'\n');
    }
    format!("sha256:{}", hex(&Sha256::digest(&text)))
}
