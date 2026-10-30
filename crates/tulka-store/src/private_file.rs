//! Files that only their owner may read.

use std::path::Path;

/// Creates the file readable by its owner only, before SQLite opens it, so the
/// contents are never exposed to other users even briefly.
pub(crate) fn create_private(path: &Path) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map(|_| ())
}

/// Creates the data folder (and its parents) and leaves it reachable by its
/// owner only, so the files inside are not listable or openable by others
/// even if one of them was created with a looser mode.
pub(crate) fn ensure_private_dir(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
}

/// Makes an existing file (and its sidecars, if any) private to the owner,
/// whatever mode an earlier version or another tool left on it.
pub(crate) fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut files = vec![path.to_path_buf()];
        for suffix in ["-wal", "-shm"] {
            let mut s = path.as_os_str().to_owned();
            s.push(suffix);
            files.push(std::path::PathBuf::from(s));
        }
        for file in files {
            if let Ok(meta) = std::fs::metadata(&file) {
                // Only the owner's bits stay, whatever they are: a file the
                // user locked (read-only or unreadable) is not reopened.
                let mode = meta.permissions().mode() & 0o700;
                let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn new_directory_is_private() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("a").join("data");
        ensure_private_dir(&dir);
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn existing_directory_is_made_private() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("data");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir);
        assert_eq!(mode(&dir), 0o700);
    }
}
