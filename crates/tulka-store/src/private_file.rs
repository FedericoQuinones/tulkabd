//! Files that only their owner may read.

use std::path::{Path, PathBuf};

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
            files.push(PathBuf::from(s));
        }
        for file in files {
            if let Ok(meta) = std::fs::metadata(&file) {
                // A file the user locked (read-only or unreadable) is left
                // as it is: restoring access would undo that.
                if meta.permissions().mode() & 0o600 == 0o600 {
                    let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}
