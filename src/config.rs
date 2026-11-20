//! The user config (`${XDG_CONFIG_HOME:-$HOME/.config}/skiman/config.toml`):
//! where the canon is and which binary the hooks call. Only `init` writes it.

use std::path::PathBuf;

use crate::failure::Failure;
use crate::fsutil;
use crate::paths::Paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserConfig {
    /// Absolute path of the canon, as given to `init`.
    pub repo: PathBuf,
    /// Absolute path skiman was invoked through at `init`, not canonicalised.
    pub binary: PathBuf,
}

impl UserConfig {
    /// Reads the user config; a missing file means skiman is not set up yet.
    pub fn load(paths: &Paths) -> Result<UserConfig, Failure> {
        let file = paths.config_file();
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Failure::operational(format!(
                    "{} does not exist; run `skiman init <path>` first",
                    file.display()
                )));
            }
            Err(e) => {
                return Err(Failure::operational(format!(
                    "cannot read {}: {e}",
                    file.display()
                )));
            }
        };
        let invalid = |what: &str| {
            Failure::operational(format!(
                "{}: {what}; run `skiman init <path>` again",
                file.display()
            ))
        };
        let doc: toml_edit::DocumentMut = text.parse().map_err(|_| invalid("not valid TOML"))?;
        let path_key = |key: &str| -> Result<PathBuf, Failure> {
            doc.get(key)
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .ok_or_else(|| invalid(&format!("`{key}` is missing or not an absolute path")))
        };
        Ok(UserConfig {
            repo: path_key("repo")?,
            binary: path_key("binary")?,
        })
    }

    /// Writes the user config (temporary file, then rename).
    pub fn save(&self, paths: &Paths) -> Result<(), Failure> {
        let file = paths.config_file();
        let as_str = |p: &PathBuf| {
            p.to_str().map(str::to_owned).ok_or_else(|| {
                Failure::operational(format!("path is not valid UTF-8: {}", p.display()))
            })
        };
        let mut doc = toml_edit::DocumentMut::new();
        doc["repo"] = toml_edit::value(as_str(&self.repo)?);
        doc["binary"] = toml_edit::value(as_str(&self.binary)?);
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(file.parent().expect("config file has a parent"))?;
            fsutil::write_atomic(&file, doc.to_string().as_bytes())
        };
        write().map_err(|e| Failure::operational(format!("cannot write {}: {e}", file.display())))
    }
}
