//! `.sql` files from disk: decoding and encoding, the allowed
//! paths and the read and write commands.

use std::path::Path;
use std::sync::Arc;

use tempfile::TempDir;
use tulka_app::*;
use tulka_core::{ConfigStore, MemoryStore};
use tulka_store::SessionStore;

/// `s` as UTF-16 LE with a BOM, the way SSMS saves.
fn utf16le_bom(s: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    out.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
    out
}

fn utf16be_bom(s: &str) -> Vec<u8> {
    let mut out = vec![0xFE, 0xFF];
    out.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
    out
}

fn utf8_bom(s: &str) -> Vec<u8> {
    [&[0xEF, 0xBB, 0xBF][..], s.as_bytes()].concat()
}

/// Non-ASCII, a character outside the BMP (a surrogate pair in UTF-16) and
/// CRLF line endings.
const SSMS: &str = "SELECT N'Año ✓ 😀'\r\nFROM dbo.Pedidos\r\nGO\r\n";
const SSMS_TEXT: &str = "SELECT N'Año ✓ 😀'\nFROM dbo.Pedidos\nGO\n";

// ------------------------------------------------------------ decoding

#[test]
fn utf16_le_with_bom_and_crlf_decodes_and_encodes_back_to_the_same_bytes() {
    let bytes = utf16le_bom(SSMS);
    let file = decode_text(&bytes).unwrap();
    assert_eq!(file.text, SSMS_TEXT);
    assert_eq!(file.encoding, FileEncoding::Utf16Le);
    assert_eq!(file.line_ending, LineEnding::Crlf);
    assert_eq!(
        encode_text(&file.text, file.encoding, file.line_ending),
        bytes
    );
}

#[test]
fn utf16_be_with_bom_round_trips() {
    let bytes = utf16be_bom(SSMS);
    let file = decode_text(&bytes).unwrap();
    assert_eq!(file.text, SSMS_TEXT);
    assert_eq!(file.encoding, FileEncoding::Utf16Be);
    assert_eq!(file.line_ending, LineEnding::Crlf);
    assert_eq!(
        encode_text(&file.text, file.encoding, file.line_ending),
        bytes
    );
}

#[test]
fn utf8_with_bom_round_trips() {
    let bytes = utf8_bom(SSMS);
    let file = decode_text(&bytes).unwrap();
    assert_eq!(file.text, SSMS_TEXT);
    assert_eq!(file.encoding, FileEncoding::Utf8Bom);
    assert_eq!(file.line_ending, LineEnding::Crlf);
    assert_eq!(
        encode_text(&file.text, file.encoding, file.line_ending),
        bytes
    );
}

#[test]
fn utf8_without_bom_round_trips() {
    let bytes = SSMS_TEXT.as_bytes();
    let file = decode_text(bytes).unwrap();
    assert_eq!(file.text, SSMS_TEXT);
    assert_eq!(file.encoding, FileEncoding::Utf8);
    assert_eq!(file.line_ending, LineEnding::Lf);
    assert_eq!(
        encode_text(&file.text, file.encoding, file.line_ending),
        bytes
    );

    let crlf = decode_text(SSMS.as_bytes()).unwrap();
    assert_eq!(crlf.text, SSMS_TEXT);
    assert_eq!(
        (crlf.encoding, crlf.line_ending),
        (FileEncoding::Utf8, LineEnding::Crlf)
    );
    assert_eq!(
        encode_text(&crlf.text, crlf.encoding, crlf.line_ending),
        SSMS.as_bytes()
    );
}

#[test]
fn empty_files_are_text() {
    let empty = decode_text(b"").unwrap();
    assert_eq!(
        (empty.text.as_str(), empty.encoding, empty.line_ending),
        ("", FileEncoding::Utf8, LineEnding::Lf)
    );
    let bom_only = decode_text(&[0xFF, 0xFE]).unwrap();
    assert_eq!(
        (bom_only.text.as_str(), bom_only.encoding),
        ("", FileEncoding::Utf16Le)
    );
    assert_eq!(
        encode_text("", FileEncoding::Utf16Le, LineEnding::Crlf),
        [0xFF, 0xFE]
    );
}

#[test]
fn the_most_frequent_line_ending_wins_and_crlf_becomes_lf_inside() {
    let file = decode_text(b"a\r\nb\r\nc\nd").unwrap();
    assert_eq!(file.line_ending, LineEnding::Crlf);
    assert_eq!(file.text, "a\nb\nc\nd");
    let file = decode_text(b"a\nb\nc\r\nd").unwrap();
    assert_eq!(file.line_ending, LineEnding::Lf);
    assert_eq!(file.text, "a\nb\nc\nd");
    // Written back, every line ends the same way.
    assert_eq!(
        encode_text("a\nb\n", FileEncoding::Utf8, LineEnding::Crlf),
        b"a\r\nb\r\n"
    );
    assert_eq!(
        encode_text("a\nb\n", FileEncoding::Utf8, LineEnding::Lf),
        b"a\nb\n"
    );
}

#[test]
fn binary_bytes_are_not_text() {
    // A PNG header: not UTF-8, no BOM.
    let png = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
    ];
    assert_eq!(decode_text(&png).unwrap_err(), NOT_TEXT);
    // Valid UTF-8 with NUL bytes: UTF-16 without a BOM, or a binary file.
    let no_bom: Vec<u8> = "SELECT 1"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(decode_text(&no_bom).unwrap_err(), NOT_TEXT);
    // UTF-8 after a UTF-8 BOM must still be valid.
    assert_eq!(
        decode_text(&[0xEF, 0xBB, 0xBF, 0xFF]).unwrap_err(),
        NOT_TEXT
    );
}

#[test]
fn malformed_utf16_is_not_text() {
    // An odd number of bytes after the BOM.
    let mut odd = utf16le_bom("SELECT 1");
    odd.push(0x41);
    assert_eq!(decode_text(&odd).unwrap_err(), NOT_TEXT);
    // An unpaired surrogate.
    assert_eq!(
        decode_text(&[0xFF, 0xFE, 0x00, 0xD8, 0x41, 0x00]).unwrap_err(),
        NOT_TEXT
    );
    // UTF-32 LE starts like UTF-16 LE, then has NULs.
    assert_eq!(
        decode_text(&[0xFF, 0xFE, 0x00, 0x00, 0x41, 0x00, 0x00, 0x00]).unwrap_err(),
        NOT_TEXT
    );
}

#[test]
fn encodings_and_line_endings_serialize_as_the_ui_expects() {
    let names: Vec<String> = [
        FileEncoding::Utf8,
        FileEncoding::Utf8Bom,
        FileEncoding::Utf16Le,
        FileEncoding::Utf16Be,
    ]
    .iter()
    .map(|e| serde_json::to_string(e).unwrap())
    .collect();
    assert_eq!(
        names,
        ["\"utf8\"", "\"utf8Bom\"", "\"utf16Le\"", "\"utf16Be\""]
    );
    assert_eq!(serde_json::to_string(&LineEnding::Lf).unwrap(), "\"lf\"");
    assert_eq!(
        serde_json::to_string(&LineEnding::Crlf).unwrap(),
        "\"crlf\""
    );
    let back: FileEncoding = serde_json::from_str("\"utf16Le\"").unwrap();
    assert_eq!(back, FileEncoding::Utf16Le);
}

// ------------------------------------------------------------ commands

fn state(dir: &TempDir) -> AppState {
    AppState::new(
        ConfigStore::at(dir.path().join("connections.json")),
        Arc::new(MemoryStore::default()),
        vec![],
    )
    .unwrap()
}

fn path_string(p: &Path) -> String {
    p.to_str().unwrap().to_string()
}

#[tokio::test]
async fn paths_that_were_not_allowed_are_neither_read_nor_written() {
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    let file = dir.path().join("q.sql");
    std::fs::write(&file, "SELECT 1").unwrap();
    let path = path_string(&file);

    assert_eq!(
        read_sql_file(&st, path.clone()).await.unwrap_err(),
        NOT_ALLOWED
    );
    let err = write_sql_file(
        &st,
        path.clone(),
        "DROP TABLE t".into(),
        FileEncoding::Utf8,
        LineEnding::Lf,
    )
    .await;
    assert_eq!(err.unwrap_err(), NOT_ALLOWED);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "SELECT 1");
}

#[tokio::test]
async fn paths_allowed_in_a_previous_run_are_allowed_after_restart() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("session.db");
    let file = dir.path().join("kept.sql");
    std::fs::write(&file, "SELECT 1").unwrap();
    let path = path_string(&file);
    {
        let st = state(&dir).with_session(Arc::new(SessionStore::open(&db)));
        let accepted = allow_paths(&st, vec![path.clone()]);
        assert_eq!(accepted, std::slice::from_ref(&path));
    }
    let st = state(&dir).with_session(Arc::new(SessionStore::open(&db)));
    assert_eq!(
        read_sql_file(&st, path.clone()).await.unwrap().text,
        "SELECT 1"
    );
    assert_eq!(
        sql_files_exist(&st, vec![path]).await.unwrap(),
        [Some(true)]
    );
}

#[tokio::test]
async fn a_path_never_allowed_is_refused() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("session.db");
    let allowed = dir.path().join("a.sql");
    let other = dir.path().join("b.sql");
    std::fs::write(&allowed, "SELECT 1").unwrap();
    std::fs::write(&other, "SELECT 2").unwrap();
    {
        let st = state(&dir).with_session(Arc::new(SessionStore::open(&db)));
        allow_paths(&st, vec![path_string(&allowed)]);
    }
    let st = state(&dir).with_session(Arc::new(SessionStore::open(&db)));
    let other = path_string(&other);
    assert_eq!(
        read_sql_file(&st, other.clone()).await.unwrap_err(),
        NOT_ALLOWED
    );
    assert_eq!(
        sql_files_exist(&st, vec![other]).await.unwrap(),
        [Some(false)]
    );
}

#[tokio::test]
async fn a_file_tab_in_the_saved_session_does_not_allow_its_path() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("session.db");
    let file = dir.path().join("x.sql");
    std::fs::write(&file, "SELECT 1").unwrap();
    let path = path_string(&file);
    {
        let st = state(&dir).with_session(Arc::new(SessionStore::open(&db)));
        let json = serde_json::json!({
            "version": 1,
            "tabs": [{ "id": "t1", "file": { "path": path, "encoding": "utf8", "lineEnding": "lf" } }],
        });
        save_session(&st, json.to_string()).await.unwrap();
    }
    let st = state(&dir).with_session(Arc::new(SessionStore::open(&db)));
    // The session is JSON text, so backslashes in a Windows path are escaped.
    let quoted = serde_json::to_string(&path).unwrap();
    assert!(load_session(&st).await.unwrap().contains(&quoted));
    assert_eq!(read_sql_file(&st, path).await.unwrap_err(), NOT_ALLOWED);
}

#[test]
fn a_dropped_folder_named_like_a_sql_file_is_not_accepted() {
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    let folder = dir.path().join("looks.sql");
    std::fs::create_dir(&folder).unwrap();
    let file = dir.path().join("real.sql");
    std::fs::write(&file, "SELECT 1").unwrap();
    let missing = dir.path().join("missing.sql");
    let accepted = accept_dropped_files(
        &st,
        vec![
            path_string(&folder),
            path_string(&file),
            path_string(&missing),
        ],
    );
    assert_eq!(accepted, [path_string(&file)]);
}

#[tokio::test]
async fn a_path_that_is_read_or_written_becomes_the_newest_remembered() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(SessionStore::open(&dir.path().join("session.db")));
    let st = state(&dir).with_session(store.clone());
    let read = dir.path().join("read.sql");
    let written = dir.path().join("written.sql");
    std::fs::write(&read, "SELECT 1").unwrap();
    std::fs::write(&written, "SELECT 2").unwrap();
    let (read, written) = (path_string(&read), path_string(&written));
    allow_paths(&st, vec![read.clone(), written.clone()]);
    let others: Vec<std::path::PathBuf> =
        (0..498).map(|i| format!("/other/{i}.sql").into()).collect();
    store.remember_allowed(&others);
    assert_eq!(store.allowed_files().len(), 500);

    read_sql_file(&st, read.clone()).await.unwrap();
    write_sql_file(
        &st,
        written.clone(),
        "SELECT 3".into(),
        FileEncoding::Utf8,
        LineEnding::Lf,
    )
    .await
    .unwrap();
    // Two more paths push out the two oldest ones, which are not these.
    store.remember_allowed(&["/new/1.sql".into(), "/new/2.sql".into()]);
    let kept = store.allowed_files();
    assert!(kept.contains(&read.into()));
    assert!(kept.contains(&written.into()));
}

#[tokio::test]
async fn only_absolute_sql_paths_are_allowed() {
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    let sql = path_string(&dir.path().join("a.sql"));
    let upper = path_string(&dir.path().join("B.SQL"));
    let txt = path_string(&dir.path().join("notes.txt"));
    let bare = path_string(&dir.path().join("sql"));
    let allowed = allow_paths(
        &st,
        vec![
            sql.clone(),
            txt.clone(),
            "relative.sql".into(),
            upper.clone(),
            bare,
        ],
    );
    assert_eq!(allowed, [sql, upper]);

    std::fs::write(&txt, "secret").unwrap();
    assert_eq!(read_sql_file(&st, txt).await.unwrap_err(), NOT_ALLOWED);
    assert_eq!(
        read_sql_file(&st, "relative.sql".into()).await.unwrap_err(),
        NOT_ALLOWED
    );
}

#[tokio::test]
async fn an_ssms_file_is_read_and_written_back_in_its_encoding_and_line_endings() {
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    let file = dir.path().join("Reporte mensual.sql");
    std::fs::write(&file, utf16le_bom(SSMS)).unwrap();
    let path = path_string(&file);
    allow_paths(&st, vec![path.clone()]);

    let opened = read_sql_file(&st, path.clone()).await.unwrap();
    assert_eq!(opened.path, path);
    assert_eq!(opened.name, "Reporte mensual.sql");
    assert_eq!(opened.text, SSMS_TEXT);
    assert_eq!(
        (opened.encoding, opened.line_ending),
        (FileEncoding::Utf16Le, LineEnding::Crlf)
    );
    let json = serde_json::to_value(&opened).unwrap();
    assert_eq!(json["lineEnding"], "crlf");
    assert_eq!(json["encoding"], "utf16Le");

    let edited = format!("{SSMS_TEXT}SELECT 2\n");
    write_sql_file(
        &st,
        path.clone(),
        edited.clone(),
        opened.encoding,
        opened.line_ending,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(&file).unwrap(),
        utf16le_bom(&format!("{SSMS}SELECT 2\r\n"))
    );

    let again = read_sql_file(&st, path).await.unwrap();
    assert_eq!(again.text, edited);
    assert_eq!(
        (again.encoding, again.line_ending),
        (FileEncoding::Utf16Le, LineEnding::Crlf)
    );
    // No temporary file is left next to it.
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
}

#[tokio::test]
async fn utf8_with_bom_is_written_back_with_its_bom() {
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    let file = dir.path().join("q.sql");
    std::fs::write(&file, utf8_bom(SSMS)).unwrap();
    let path = path_string(&file);
    allow_paths(&st, vec![path.clone()]);

    let opened = read_sql_file(&st, path.clone()).await.unwrap();
    assert_eq!(
        (opened.encoding, opened.line_ending),
        (FileEncoding::Utf8Bom, LineEnding::Crlf)
    );
    write_sql_file(&st, path, opened.text, opened.encoding, opened.line_ending)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), utf8_bom(SSMS));
}

#[tokio::test]
async fn a_deleted_file_is_written_again_at_the_same_path() {
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    let file = dir.path().join("q.sql");
    std::fs::write(&file, "SELECT 1").unwrap();
    let path = path_string(&file);
    allow_paths(&st, vec![path.clone()]);
    std::fs::remove_file(&file).unwrap();

    assert_eq!(
        read_sql_file(&st, path.clone()).await.unwrap_err(),
        NOT_FOUND
    );
    write_sql_file(
        &st,
        path,
        "SELECT 2\n".into(),
        FileEncoding::Utf8,
        LineEnding::Crlf,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"SELECT 2\r\n");
}

#[tokio::test]
async fn a_binary_file_is_not_opened_and_its_bytes_stay_out_of_the_error() {
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    let file = dir.path().join("image.sql");
    std::fs::write(&file, [0x89, b'P', b'N', b'G', 0xFF, 0x00]).unwrap();
    let path = path_string(&file);
    allow_paths(&st, vec![path.clone()]);
    assert_eq!(read_sql_file(&st, path).await.unwrap_err(), NOT_TEXT);
}

#[cfg(unix)]
#[tokio::test]
async fn a_read_only_file_is_not_overwritten() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    let file = dir.path().join("q.sql");
    std::fs::write(&file, "SELECT 1").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    let path = path_string(&file);
    allow_paths(&st, vec![path.clone()]);

    let err = write_sql_file(
        &st,
        path.clone(),
        "SELECT 2".into(),
        FileEncoding::Utf8,
        LineEnding::Lf,
    )
    .await
    .unwrap_err();
    assert!(err.contains(&path), "{err}");
    assert!(!err.contains("SELECT"), "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "SELECT 1");
}

#[cfg(unix)]
#[tokio::test]
async fn a_write_keeps_the_file_permissions_and_symlinks() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    let real = dir.path().join("real.sql");
    std::fs::write(&real, "SELECT 1").unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
    let link = dir.path().join("link.sql");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let path = path_string(&link);
    allow_paths(&st, vec![path.clone()]);

    write_sql_file(
        &st,
        path,
        "SELECT 2".into(),
        FileEncoding::Utf8,
        LineEnding::Lf,
    )
    .await
    .unwrap();
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "SELECT 2");
    assert_eq!(
        std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[tokio::test]
async fn sql_files_exist_reports_allowed_files_that_are_still_there() {
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    let here = dir.path().join("here.sql");
    std::fs::write(&here, "SELECT 1").unwrap();
    let gone = dir.path().join("gone.sql");
    let folder = dir.path().join("folder.sql");
    std::fs::create_dir(&folder).unwrap();
    let other = dir.path().join("other.sql");
    std::fs::write(&other, "SELECT 1").unwrap();
    let paths: Vec<String> = [&here, &gone, &folder]
        .iter()
        .map(|p| path_string(p))
        .collect();
    allow_paths(&st, paths.clone());

    let mut asked = paths;
    // Never allowed: reported missing, whatever is on disk.
    asked.push(path_string(&other));
    assert_eq!(
        sql_files_exist(&st, asked).await.unwrap(),
        [Some(true), Some(false), Some(false), Some(false)]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn sql_files_exist_tells_a_file_it_could_not_check_from_a_missing_one() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    let inside = locked.join("q.sql");
    std::fs::write(&inside, "SELECT 1").unwrap();
    let under_a_file = dir.path().join("plain").join("q.sql");
    std::fs::write(dir.path().join("plain"), "not a folder").unwrap();
    let paths = vec![path_string(&inside), path_string(&under_a_file)];
    allow_paths(&st, paths.clone());

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads through any permission: nothing to check then.
    let enforced = std::fs::metadata(&inside).is_err();
    let found = sql_files_exist(&st, paths).await.unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if enforced {
        assert_eq!(found, [None, Some(false)]);
    }
}

#[tokio::test]
async fn a_file_over_20_mb_is_not_read() {
    assert_eq!(MAX_SQL_FILE_BYTES, 20 * 1024 * 1024);
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    // Sparse files: nothing is really written to the disk.
    let big = dir.path().join("big.sql");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(MAX_SQL_FILE_BYTES + 1)
        .unwrap();
    let limit = dir.path().join("limit.sql");
    std::fs::File::create(&limit)
        .unwrap()
        .set_len(MAX_SQL_FILE_BYTES)
        .unwrap();
    let (big, limit) = (path_string(&big), path_string(&limit));
    allow_paths(&st, vec![big.clone(), limit.clone()]);

    assert_eq!(read_sql_file(&st, big).await.unwrap_err(), TOO_LARGE);
    // Read, then refused for its NUL bytes: the size itself is accepted.
    assert_eq!(read_sql_file(&st, limit).await.unwrap_err(), NOT_TEXT);
}

#[tokio::test]
async fn stored_paths_that_are_not_sql_files_are_not_allowed() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("session.db");
    let secret = dir.path().join("notes.txt");
    std::fs::write(&secret, "private").unwrap();
    let path = path_string(&secret);
    SessionStore::open(&db).remember_allowed(&[secret.clone(), "relative.sql".into()]);
    let st = state(&dir).with_session(Arc::new(SessionStore::open(&db)));
    assert_eq!(read_sql_file(&st, path).await.unwrap_err(), NOT_ALLOWED);
}
