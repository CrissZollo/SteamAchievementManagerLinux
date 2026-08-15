//! Locating the Steam installation and its client library.
//!
//! The Windows original reads `HKLM\Software\Valve\Steam\InstallPath` from the
//! registry. On Linux there is no registry, so we probe the handful of layouts
//! Valve and the distributions actually use.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Relative paths to `steamclient.so`, most preferred first.
///
/// A 64-bit build must load the 64-bit library: the loader will refuse to map
/// an ELF of the wrong class, and even if it did, every vtable offset would be
/// wrong. `linux64` is the modern location; `steamrt64` appears on installs
/// that have migrated to the Steam Runtime layout.
#[cfg(target_pointer_width = "64")]
const CLIENT_LIBRARY_CANDIDATES: &[&str] = &[
    "linux64/steamclient.so",
    "steamrt64/steamclient.so",
    "ubuntu12_64/steamclient.so",
];

#[cfg(target_pointer_width = "32")]
const CLIENT_LIBRARY_CANDIDATES: &[&str] = &[
    "linux32/steamclient.so",
    "steamrt32/steamclient.so",
    "ubuntu12_32/steamclient.so",
];

/// Candidate Steam roots, most preferred first. Several of these are normally
/// symlinks to the same directory; duplicates are filtered after canonicalising.
fn candidate_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    // An explicit override always wins. Useful for unusual installs and for
    // pointing the tool at a second Steam library on another drive.
    for var in ["SAM_STEAM_ROOT", "STEAM_ROOT", "STEAM_BASE_FOLDER"] {
        if let Some(value) = std::env::var_os(var) {
            if !value.is_empty() {
                roots.push(PathBuf::from(value));
            }
        }
    }

    let Some(home) = home_dir() else {
        return roots;
    };

    roots.extend(
        [
            ".steam/steam",                                        // canonical symlink
            ".steam/root",                                         // alternative symlink
            ".local/share/Steam",                                  // native default
            ".steam/debian-installation",                          // Debian/Ubuntu package
            ".var/app/com.valvesoftware.Steam/.local/share/Steam", // Flatpak
            ".steam",
        ]
        .iter()
        .map(|rel| home.join(rel)),
    );

    roots
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// A located Steam installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamPaths {
    root: PathBuf,
    client_library: PathBuf,
}

impl SteamPaths {
    /// Find Steam, requiring that a loadable `steamclient.so` of the right
    /// architecture is present. A root without one is skipped rather than
    /// accepted, so a stale `~/.steam` cannot mask a working install.
    pub fn discover() -> Result<Self> {
        let mut seen: Vec<PathBuf> = Vec::new();
        let mut searched: Vec<PathBuf> = Vec::new();
        let mut found_any_root = false;

        for root in candidate_roots() {
            let root = match root.canonicalize() {
                Ok(r) => r,
                Err(_) => continue,
            };
            if !root.is_dir() || seen.contains(&root) {
                continue;
            }
            seen.push(root.clone());
            found_any_root = true;

            for rel in CLIENT_LIBRARY_CANDIDATES {
                let candidate = root.join(rel);
                if candidate.is_file() {
                    return Ok(Self {
                        root,
                        client_library: candidate,
                    });
                }
                searched.push(candidate);
            }
        }

        if !found_any_root {
            return Err(Error::SteamNotFound);
        }
        Err(Error::ClientLibraryNotFound { searched })
    }

    /// Build from an explicit root, for tests and for `--steam-root`.
    pub fn from_root(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        let mut searched = Vec::new();
        for rel in CLIENT_LIBRARY_CANDIDATES {
            let candidate = root.join(rel);
            if candidate.is_file() {
                return Ok(Self {
                    root,
                    client_library: candidate,
                });
            }
            searched.push(candidate);
        }
        Err(Error::ClientLibraryNotFound { searched })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn client_library(&self) -> &Path {
        &self.client_library
    }

    /// Directory holding `UserGameStatsSchema_<appid>.bin`.
    pub fn stats_cache_dir(&self) -> PathBuf {
        self.root.join("appcache").join("stats")
    }

    /// The cached stat and achievement schema for an app, if Steam has one.
    ///
    /// Steam writes this the first time an app with stats is launched, so a
    /// game that has never been run will not have one.
    pub fn schema_path(&self, app_id: u32) -> PathBuf {
        self.stats_cache_dir()
            .join(format!("UserGameStatsSchema_{app_id}.bin"))
    }

    /// App IDs that have a cached schema, i.e. every app on this machine known
    /// to have achievements or stats. This is the offline half of game
    /// discovery and needs no network access.
    pub fn apps_with_cached_schema(&self) -> Vec<u32> {
        let Ok(entries) = std::fs::read_dir(self.stats_cache_dir()) else {
            return Vec::new();
        };

        let mut ids: Vec<u32> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name();
                let name = name.to_str()?;
                name.strip_prefix("UserGameStatsSchema_")?
                    .strip_suffix(".bin")?
                    .parse()
                    .ok()
            })
            .collect();

        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_root_is_reported() {
        let err = SteamPaths::from_root("/nonexistent-steam-root-for-tests");
        assert!(err.is_err());
    }

    #[test]
    fn discovery_matches_this_machine_when_steam_is_present() {
        // Informational rather than assertive: CI has no Steam install.
        match SteamPaths::discover() {
            Ok(paths) => {
                assert!(paths.client_library().is_file());
                assert!(paths.root().is_dir());
            }
            Err(e) => eprintln!("no Steam on this machine ({e}); skipping"),
        }
    }
}
