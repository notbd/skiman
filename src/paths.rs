//! Every path skiman uses derives from `$HOME` and the XDG variables, never
//! from platform directories (spec: Language and packaging).

use std::path::{Path, PathBuf};

use crate::failure::Failure;

/// Base directories read from the environment once, at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: PathBuf,
    config_home: PathBuf,
    state_home: PathBuf,
}

impl Paths {
    /// Reads `HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME` from the process
    /// environment. An XDG variable that is unset, empty or relative falls back
    /// to its default under `HOME`, as the XDG Base Directory spec requires.
    /// Exit 4 when `HOME` is unset, empty or relative.
    pub fn from_env() -> Result<Self, Failure> {
        let home = PathBuf::from(
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .ok_or_else(|| Failure::operational("HOME is not set"))?,
        );
        if !home.is_absolute() {
            return Err(Failure::operational(format!(
                "HOME is not an absolute path: {}",
                home.display()
            )));
        }
        let xdg = |name: &str, default: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(default))
        };
        Ok(Paths {
            config_home: xdg("XDG_CONFIG_HOME", ".config"),
            state_home: xdg("XDG_STATE_HOME", ".local/state"),
            home,
        })
    }

    /// Paths under `home` with the XDG defaults, for unit tests.
    #[cfg(test)]
    pub fn under(home: &Path) -> Paths {
        Paths {
            home: home.to_path_buf(),
            config_home: home.join(".config"),
            state_home: home.join(".local/state"),
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `${XDG_CONFIG_HOME:-$HOME/.config}/skiman/config.toml`.
    pub fn config_file(&self) -> PathBuf {
        self.config_home.join("skiman/config.toml")
    }

    /// `${XDG_STATE_HOME:-$HOME/.local/state}/skiman`: machine state.
    pub fn state_dir(&self) -> PathBuf {
        self.state_home.join("skiman")
    }
}
