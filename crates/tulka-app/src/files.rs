//! `.sql` files from disk (plan D7).
//!
//! # Text
//!
//! A file is read as UTF-8 (with or without a BOM) or as UTF-16 LE or BE
//! with a BOM, the encodings SSMS saves in; anything else, or text with NUL
//! characters (UTF-16 without a BOM, binary files), is [`NOT_TEXT`]. The
//! editor gets the text with `\n` line breaks; [`TextFile`] keeps the
//! encoding and the more frequent line ending (LF on a tie) so
//! [`encode_text`] writes it back the same way. A file larger than
//! [`MAX_SQL_FILE_BYTES`] is [`TOO_LARGE`] and is not read at all.
//!
//! # Allowed paths
//!
//! The backend only reads or writes the paths the user picked in the open
//! dialog or dropped on the window in this session, or that came back with
//! the restored session (the UI passes those last two to [`allow_paths`]).
//! Only absolute paths ending in `.sql` are ever allowed. Any other path
//! fails with [`NOT_ALLOWED`].
//!
//! # Writes
//!
//! Atomic: a temporary file next to the target, then a rename. A file
//! deleted since it was opened is written again at the same path; a
//! symlink is written through to its target and stays a symlink; the
//! file's permissions are kept; a read-only file is not overwritten.
//!
//! The file text is user data: it is never logged nor put in an error.
//! Errors may name the path.

use std::borrow::Cow;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::connections::blocking;
use crate::queries::write_atomic;
use crate::state::AppState;
use crate::types::{NOT_ALLOWED, NOT_FOUND, NOT_TEXT, TOO_LARGE};

/// The largest `.sql` file [`read_sql_file`] reads (20 MB); a larger one is
/// [`TOO_LARGE`], so dropping a huge file never fills the memory or the
/// editor.
pub const MAX_SQL_FILE_BYTES: u64 = 20 * 1024 * 1024;

const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
const UTF16LE_BOM: [u8; 2] = [0xFF, 0xFE];
const UTF16BE_BOM: [u8; 2] = [0xFE, 0xFF];

/// How a `.sql` file is encoded on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

/// How the lines of a `.sql` file end on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LineEnding {
    Lf,
    Crlf,
}

/// A decoded file: its text with `\n` line breaks, and how to write it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextFile {
    pub text: String,
    pub encoding: FileEncoding,
    pub line_ending: LineEnding,
}

/// A file opened in a tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedFile {
    /// As given to [`read_sql_file`]; [`write_sql_file`] takes it back.
    pub path: String,
    /// The file name, the tab's title.
    pub name: String,
    pub text: String,
    pub encoding: FileEncoding,
    pub line_ending: LineEnding,
}

/// Decodes a file (see the module docs); [`NOT_TEXT`] when it is not text.
pub fn decode_text(bytes: &[u8]) -> Result<TextFile, String> {
    let (encoding, raw): (FileEncoding, Option<Cow<str>>) = if let Some(rest) =
        bytes.strip_prefix(&UTF8_BOM)
    {
        (
            FileEncoding::Utf8Bom,
            std::str::from_utf8(rest).ok().map(Cow::Borrowed),
        )
    } else if let Some(rest) = bytes.strip_prefix(&UTF16LE_BOM) {
        let text = encoding_rs::UTF_16LE.decode_without_bom_handling_and_without_replacement(rest);
        (FileEncoding::Utf16Le, text)
    } else if let Some(rest) = bytes.strip_prefix(&UTF16BE_BOM) {
        let text = encoding_rs::UTF_16BE.decode_without_bom_handling_and_without_replacement(rest);
        (FileEncoding::Utf16Be, text)
    } else {
        (
            FileEncoding::Utf8,
            std::str::from_utf8(bytes).ok().map(Cow::Borrowed),
        )
    };
    let raw = raw.ok_or(NOT_TEXT)?;
    if raw.contains('\0') {
        return Err(NOT_TEXT.into());
    }
    let crlf = raw.matches("\r\n").count();
    let lf = raw.matches('\n').count() - crlf;
    let line_ending = if crlf > lf {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    };
    let text = if crlf > 0 {
        raw.replace("\r\n", "\n")
    } else {
        raw.into_owned()
    };
    Ok(TextFile {
        text,
        encoding,
        line_ending,
    })
}

/// `text` (with `\n` line breaks) as the bytes of a file in `encoding`,
/// with a BOM unless plain UTF-8, every line ending in `line_ending`.
pub fn encode_text(text: &str, encoding: FileEncoding, line_ending: LineEnding) -> Vec<u8> {
    let text = match line_ending {
        LineEnding::Lf => Cow::Borrowed(text),
        LineEnding::Crlf => Cow::Owned(text.replace('\n', "\r\n")),
    };
    match encoding {
        FileEncoding::Utf8 => text.into_owned().into_bytes(),
        FileEncoding::Utf8Bom => [&UTF8_BOM[..], text.as_bytes()].concat(),
        FileEncoding::Utf16Le => UTF16LE_BOM
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect(),
        FileEncoding::Utf16Be => UTF16BE_BOM
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
            .collect(),
    }
}

/// The paths the file commands may touch (see the module docs).
#[derive(Default)]
pub(crate) struct AllowedFiles(Mutex<HashSet<PathBuf>>);

impl AllowedFiles {
    fn contains(&self, path: &Path) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(path)
    }
}

fn is_sql_file_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|e| e.eq_ignore_ascii_case("sql"))
}

/// Lets the file commands read and write `paths`: ones the user picked,
/// dropped on the window or that came back with the restored session.
/// Returns those accepted, in order: absolute paths ending in `.sql`.
pub fn allow_paths(state: &AppState, paths: Vec<String>) -> Vec<String> {
    let accepted: Vec<String> = paths
        .into_iter()
        .filter(|p| is_sql_file_path(Path::new(p)))
        .collect();
    let mut set = state.files.0.lock().unwrap_or_else(PoisonError::into_inner);
    set.extend(accepted.iter().map(PathBuf::from));
    accepted
}

fn check_allowed(state: &AppState, path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if state.files.contains(&path) {
        Ok(path)
    } else {
        Err(NOT_ALLOWED.into())
    }
}

/// Reads an allowed `.sql` file: [`NOT_ALLOWED`], [`NOT_FOUND`] when it no
/// longer exists, [`TOO_LARGE`] over [`MAX_SQL_FILE_BYTES`], [`NOT_TEXT`].
pub async fn read_sql_file(state: &AppState, path: String) -> Result<OpenedFile, String> {
    let p = check_allowed(state, &path)?;
    blocking(move || {
        let fail = |e: std::io::Error| match e.kind() {
            ErrorKind::NotFound => NOT_FOUND.to_string(),
            _ => format!("could not read {}: {e}", p.display()),
        };
        if fs::metadata(&p).map_err(fail)?.len() > MAX_SQL_FILE_BYTES {
            return Err(TOO_LARGE.to_string());
        }
        let bytes = fs::read(&p).map_err(fail)?;
        let file = decode_text(&bytes)?;
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(OpenedFile {
            path,
            name,
            text: file.text,
            encoding: file.encoding,
            line_ending: file.line_ending,
        })
    })
    .await?
}

/// For each path: `Some(true)` when it is allowed and a file is there now
/// (through a symlink too), `Some(false)` when it is not (nothing there, a
/// folder, or a path that is not allowed, so nothing is learnt about other
/// paths), `None` when it could not be checked (no permission, an I/O
/// error). Fails as a whole only when the check could not run. The UI checks
/// restored file tabs with it and keeps the file of a tab it could not check.
pub async fn sql_files_exist(
    state: &AppState,
    paths: Vec<String>,
) -> Result<Vec<Option<bool>>, String> {
    let allowed: Vec<bool> = paths
        .iter()
        .map(|p| state.files.contains(Path::new(p)))
        .collect();
    blocking(move || {
        paths
            .iter()
            .zip(allowed)
            .map(|(p, ok)| {
                if ok {
                    file_exists(Path::new(p))
                } else {
                    Some(false)
                }
            })
            .collect()
    })
    .await
}

/// Whether a file is at `path`, or `None` when that cannot be told.
fn file_exists(path: &Path) -> Option<bool> {
    match fs::metadata(path) {
        Ok(meta) => Some(meta.is_file()),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => Some(false),
        Err(_) => None,
    }
}

/// Writes `text` to an allowed `.sql` file in `encoding` and `line_ending`,
/// atomically: [`NOT_ALLOWED`], or why it could not be written.
pub async fn write_sql_file(
    state: &AppState,
    path: String,
    text: String,
    encoding: FileEncoding,
    line_ending: LineEnding,
) -> Result<(), String> {
    let p = check_allowed(state, &path)?;
    blocking(move || {
        let fail = |e: &dyn std::fmt::Display| format!("could not save {}: {e}", p.display());
        let bytes = encode_text(&text, encoding, line_ending);
        // Through a symlink to its target, so the link stays; the path
        // itself when the file was deleted.
        let target = fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
        let permissions = match fs::metadata(&target) {
            Ok(meta) if meta.permissions().readonly() => return Err(fail(&"it is read-only")),
            Ok(meta) => Some(meta.permissions()),
            Err(_) => None,
        };
        let dir = target.parent().ok_or_else(|| fail(&"it has no folder"))?;
        write_atomic(dir, &target, &bytes, permissions).map_err(|e| fail(&e))
    })
    .await?
}
