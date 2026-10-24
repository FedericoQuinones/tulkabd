//! Where tulkabd keeps its files, for the Settings screen. Pure path logic:
//! opening a folder in the system file manager needs a plugin and lives in
//! the Tauri layer.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::types::NOT_FOUND;

/// The files and folders tulkabd keeps for the user, as paths to show.
/// A path is empty when the platform has no such directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataLocations {
    pub config_file: String,
    pub data_dir: String,
    pub queries_dir: String,
    pub cache_file: String,
    pub run_log_file: String,
    pub session_file: String,
}

/// The locations under a config directory and a data directory.
pub fn data_locations_in(config_dir: &Path, data_dir: &Path) -> DataLocations {
    let show = |p: PathBuf| p.to_string_lossy().into_owned();
    DataLocations {
        config_file: show(config_dir.join("config.json")),
        data_dir: show(data_dir.to_path_buf()),
        queries_dir: show(data_dir.join("queries")),
        cache_file: show(data_dir.join("tulkabd.db")),
        run_log_file: show(data_dir.join("runlog.db")),
        session_file: show(data_dir.join("session.db")),
    }
}

/// The per-user locations of this platform.
pub fn data_locations() -> DataLocations {
    match directories::ProjectDirs::from("dev", "tulkabd", "tulkabd") {
        Some(dirs) => data_locations_in(dirs.config_dir(), dirs.data_dir()),
        None => DataLocations {
            config_file: String::new(),
            data_dir: String::new(),
            queries_dir: String::new(),
            cache_file: String::new(),
            run_log_file: String::new(),
            session_file: String::new(),
        },
    }
}

/// What to show in the file manager for `which` (`"config"`, `"queries"` or
/// `"data"`) among `locations`. Folders are created if missing, so there is
/// always something to show; the config shows its file, or its folder when
/// the file was never written. Fails with `"not_found"` for an unknown key or
/// a location the platform does not have.
pub fn reveal_target_in(locations: &DataLocations, which: &str) -> Result<PathBuf, String> {
    let (path, is_dir) = match which {
        "config" => (&locations.config_file, false),
        "queries" => (&locations.queries_dir, true),
        "data" => (&locations.data_dir, true),
        _ => return Err(NOT_FOUND.into()),
    };
    if path.is_empty() {
        return Err(NOT_FOUND.into());
    }
    let path = PathBuf::from(path);
    let dir = if is_dir {
        Some(path.as_path())
    } else {
        path.parent()
    };
    if path.exists() {
        return Ok(path);
    }
    let dir = dir.ok_or_else(|| NOT_FOUND.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create the folder: {e}"))?;
    Ok(if is_dir { path } else { dir.to_path_buf() })
}

/// [`reveal_target_in`] for this platform's locations.
pub fn reveal_target(which: &str) -> Result<PathBuf, String> {
    reveal_target_in(&data_locations(), which)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_sit_under_the_two_directories() {
        let dir = tempfile::tempdir().unwrap();
        let (cfg, data) = (dir.path().join("cfg"), dir.path().join("data"));
        let l = data_locations_in(&cfg, &data);
        assert_eq!(PathBuf::from(&l.config_file), cfg.join("config.json"));
        assert_eq!(PathBuf::from(&l.data_dir), data);
        assert_eq!(PathBuf::from(&l.queries_dir), data.join("queries"));
        assert_eq!(PathBuf::from(&l.cache_file), data.join("tulkabd.db"));
        assert_eq!(PathBuf::from(&l.run_log_file), data.join("runlog.db"));
        assert_eq!(PathBuf::from(&l.session_file), data.join("session.db"));
    }

    #[test]
    fn serializes_camel_case() {
        let l = data_locations_in(Path::new("c"), Path::new("d"));
        let v = serde_json::to_value(&l).unwrap();
        assert!(v.get("configFile").is_some() && v.get("runLogFile").is_some());
    }

    #[test]
    fn reveal_creates_the_folders_and_shows_the_config_file_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let (cfg, data) = (dir.path().join("cfg"), dir.path().join("data"));
        let l = data_locations_in(&cfg, &data);

        // The config was never written: its folder is shown, and now exists.
        assert_eq!(reveal_target_in(&l, "config").unwrap(), cfg);
        assert!(cfg.is_dir());
        std::fs::write(cfg.join("config.json"), "{}").unwrap();
        assert_eq!(
            reveal_target_in(&l, "config").unwrap(),
            cfg.join("config.json")
        );

        assert_eq!(
            reveal_target_in(&l, "queries").unwrap(),
            data.join("queries")
        );
        assert!(data.join("queries").is_dir());
        assert_eq!(reveal_target_in(&l, "data").unwrap(), data);
    }

    #[test]
    fn reveal_refuses_unknown_keys_and_missing_locations() {
        let dir = tempfile::tempdir().unwrap();
        let l = data_locations_in(dir.path(), dir.path());
        assert_eq!(reveal_target_in(&l, "../etc").unwrap_err(), NOT_FOUND);
        let none = DataLocations {
            config_file: String::new(),
            data_dir: String::new(),
            queries_dir: String::new(),
            cache_file: String::new(),
            run_log_file: String::new(),
            session_file: String::new(),
        };
        assert_eq!(reveal_target_in(&none, "data").unwrap_err(), NOT_FOUND);
    }
}
