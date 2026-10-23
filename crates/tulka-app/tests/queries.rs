//! Saved queries on disk: name encoding, the store and the commands.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;
use tulka_app::*;
use tulka_core::{ConfigStore, MemoryStore};

/// The odd names of the review: Windows-forbidden characters, separators,
/// reserved device names, a trailing dot, non-ASCII, `%` and `..`.
const ODD: &[&str] = &[
    "Ventas:2024",
    "a/b",
    "a\\b",
    "CON",
    "con.sql",
    "Pedidos.",
    "Año 2024",
    "100%",
    "%",
    "..",
    ".",
    "nul",
    "COM1",
    "lpt9.txt",
    "x ",
    "<>\"|?*",
    "tab\there",
    "Reporte mensual",
];

fn store() -> (TempDir, QueriesStore) {
    let dir = TempDir::new().unwrap();
    let store = QueriesStore::at(dir.path().join("queries"));
    (dir, store)
}

/// Every path under `dir`, relative to it.
fn tree(dir: &Path) -> Vec<PathBuf> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            out.push(p.strip_prefix(base).unwrap().to_path_buf());
            if p.is_dir() {
                walk(base, &p, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn names(list: &[SavedQuery]) -> Vec<(&str, &str)> {
    list.iter()
        .map(|q| (q.database.as_str(), q.name.as_str()))
        .collect()
}

// ------------------------------------------------------------ encoding

#[test]
fn odd_names_round_trip() {
    for name in ODD {
        let enc = encode_component(name);
        assert_eq!(
            decode_component(&enc).as_deref(),
            Some(*name),
            "{name:?} -> {enc:?}"
        );
    }
}

#[test]
fn encoded_names_are_safe_file_names() {
    const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL", "COM1", "LPT9"];
    for name in ODD {
        let enc = encode_component(name);
        assert!(!enc.is_empty());
        assert_ne!(enc, ".");
        assert_ne!(enc, "..");
        assert!(
            !enc.chars()
                .any(|c| "<>:\"/\\|?*".contains(c) || c.is_control()),
            "{name:?} -> {enc:?}"
        );
        assert!(!enc.ends_with('.') && !enc.ends_with(' '), "{enc:?}");
        let stem = enc.split('.').next().unwrap().trim_end().to_uppercase();
        assert!(!RESERVED.contains(&stem.as_str()), "{name:?} -> {enc:?}");
    }
}

#[test]
fn specific_encodings() {
    assert_ne!(encode_component(".."), "..");
    assert_eq!(encode_component("Ventas:2024"), "Ventas%3A2024");
    assert_eq!(encode_component("a/b"), "a%2Fb");
    assert_eq!(encode_component("100%"), "100%25");
    assert_eq!(encode_component("Pedidos."), "Pedidos%2E");
    assert_eq!(encode_component("Año 2024"), "Año 2024");
    assert_eq!(encode_component("Reporte mensual"), "Reporte mensual");
    assert_ne!(encode_component("CON").to_uppercase(), "CON");
}

#[test]
fn bad_encodings_decode_to_none() {
    for bad in [
        "a%", "a%2", "a%zz", "a%2f", "a:b", "a%41", "Pedidos.", "CON", "", "%C3%B1",
    ] {
        assert_eq!(decode_component(bad), None, "{bad:?}");
    }
}

#[test]
fn names_differing_in_one_forbidden_character_do_not_collide() {
    let names = [
        "a:b", "a/b", "a\\b", "a|b", "a?b", "a*b", "a<b", "a>b", "a\"b", "a%b", "a_b", "a%3Ab",
        "a%253Ab",
    ];
    let encoded: HashSet<String> = names.iter().map(|n| encode_component(n)).collect();
    assert_eq!(encoded.len(), names.len());
}

// ------------------------------------------------------------ store

#[test]
fn save_then_list_shows_it() {
    let (_dir, s) = store();
    s.save("Ventas", "Reporte mensual", "SELECT 1", false)
        .unwrap();
    s.save("Compras", "Otra", "SELECT 2", false).unwrap();
    let list = s.list(Some("Ventas"));
    assert_eq!(names(&list), [("Ventas", "Reporte mensual")]);
    assert!(list[0].modified_ms > 0);
    assert_eq!(s.read("Ventas", "Reporte mensual").unwrap(), "SELECT 1");
    assert_eq!(
        names(&s.list(None)),
        [("Compras", "Otra"), ("Ventas", "Reporte mensual")]
    );
}

#[test]
fn database_names_are_case_sensitive() {
    let (_dir, s) = store();
    s.save("Ventas", "q", "SELECT 1", false).unwrap();
    assert!(s.list(Some("ventas")).is_empty());
}

#[test]
fn save_without_overwrite_on_existing_fails_with_exists() {
    let (_dir, s) = store();
    s.save("Ventas", "q", "SELECT 1", false).unwrap();
    assert_eq!(
        s.save("Ventas", "q", "SELECT 2", false),
        Err("exists".into())
    );
    assert_eq!(s.read("Ventas", "q").unwrap(), "SELECT 1");
    s.save("Ventas", "q", "SELECT 2", true).unwrap();
    assert_eq!(s.read("Ventas", "q").unwrap(), "SELECT 2");
}

#[test]
fn empty_or_blank_names_are_rejected() {
    let (_dir, s) = store();
    for name in ["", "   ", "\t"] {
        assert_eq!(s.save("Ventas", name, "x", false), Err("empty_name".into()));
    }
    s.save("Ventas", "q", "x", false).unwrap();
    assert_eq!(s.rename("Ventas", "q", " "), Err("empty_name".into()));
}

#[test]
fn rename_and_delete() {
    let (_dir, s) = store();
    s.save("Ventas", "a", "SELECT 'a'", false).unwrap();
    s.save("Ventas", "b", "SELECT 'b'", false).unwrap();
    assert_eq!(s.rename("Ventas", "a", "b"), Err("exists".into()));
    s.rename("Ventas", "a", "c").unwrap();
    assert_eq!(
        names(&s.list(Some("Ventas"))),
        [("Ventas", "b"), ("Ventas", "c")]
    );
    assert_eq!(s.read("Ventas", "c").unwrap(), "SELECT 'a'");
    assert_eq!(s.read("Ventas", "a"), Err("not_found".into()));
    assert_eq!(s.rename("Ventas", "a", "d"), Err("not_found".into()));

    s.delete("Ventas", "b").unwrap();
    assert_eq!(names(&s.list(Some("Ventas"))), [("Ventas", "c")]);
    assert_eq!(s.delete("Ventas", "b"), Err("not_found".into()));
}

#[test]
fn odd_names_round_trip_through_the_disk_and_stay_inside_root() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("queries");
    let s = QueriesStore::at(root.clone());
    for (i, db) in ODD.iter().enumerate() {
        for name in ODD {
            s.save(db, name, &format!("-- {i}"), false).unwrap();
        }
    }
    let listed: HashSet<(String, String)> = s
        .list(None)
        .into_iter()
        .map(|q| (q.database, q.name))
        .collect();
    let expected: HashSet<(String, String)> = ODD
        .iter()
        .flat_map(|db| ODD.iter().map(|n| (db.to_string(), n.to_string())))
        .collect();
    assert_eq!(listed, expected);
    for db in ODD {
        assert_eq!(s.list(Some(db)).len(), ODD.len(), "{db:?}");
        for name in ODD {
            assert!(s.path_of(db, name).unwrap().starts_with(&root));
        }
    }
    // Nothing was created next to `queries/`, and every file is one level
    // down: `queries/<database>/<name>.sql`.
    assert_eq!(tree(dir.path())[0], PathBuf::from("queries"));
    assert!(tree(dir.path()).iter().all(|p| p.starts_with("queries")));
    for p in tree(&root) {
        let depth = p.components().count();
        if p.extension().is_some_and(|e| e == "sql") && depth == 2 {
            continue;
        }
        assert_eq!(depth, 1, "{p:?}");
        assert!(root.join(&p).is_dir(), "{p:?}");
    }
}

#[test]
fn list_ignores_badly_encoded_files_and_temp_files() {
    let (_dir, s) = store();
    s.save("Ventas", "ok", "x", false).unwrap();
    let db = s
        .path_of("Ventas", "ok")
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    for f in [
        "bad%zz.sql",
        "%41.sql",
        "lower%2f.sql",
        "notes.txt",
        ".ok.sql.tmp",
    ] {
        std::fs::write(db.join(f), "x").unwrap();
    }
    std::fs::create_dir_all(s.root().unwrap().join("bad%zz")).unwrap();
    std::fs::write(s.root().unwrap().join("bad%zz").join("q.sql"), "x").unwrap();
    assert_eq!(names(&s.list(None)), [("Ventas", "ok")]);
}

#[test]
fn list_of_a_missing_root_is_empty() {
    let (_dir, s) = store();
    assert!(s.list(None).is_empty());
    assert!(s.list(Some("Ventas")).is_empty());
    assert_eq!(s.read("Ventas", "q"), Err("not_found".into()));
}

#[test]
fn saves_leave_no_temp_files() {
    let (_dir, s) = store();
    s.save("Ventas", "q", "SELECT 1", false).unwrap();
    s.save("Ventas", "q", "SELECT 2", true).unwrap();
    let db = s
        .path_of("Ventas", "q")
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert_eq!(tree(&db), [PathBuf::from("q.sql")]);
}

// ------------------------------------------------------------ commands

fn state(dir: &TempDir) -> AppState {
    AppState::new(
        ConfigStore::at(dir.path().join("connections.json")),
        Arc::new(MemoryStore::default()),
        vec![],
    )
    .unwrap()
    .with_queries(QueriesStore::at(dir.path().join("queries")))
}

#[tokio::test]
async fn commands_save_list_read_rename_delete() {
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    save_query(&st, "Ventas".into(), "q".into(), "SELECT 1".into(), false)
        .await
        .unwrap();
    assert_eq!(
        save_query(&st, "Ventas".into(), "q".into(), "SELECT 2".into(), false).await,
        Err("exists".into())
    );
    let list = list_saved_queries(&st, Some("Ventas".into())).await;
    assert_eq!(names(&list), [("Ventas", "q")]);
    assert_eq!(
        read_saved_query(&st, "Ventas".into(), "q".into())
            .await
            .unwrap(),
        "SELECT 1"
    );
    rename_saved_query(&st, "Ventas".into(), "q".into(), "r".into())
        .await
        .unwrap();
    assert_eq!(
        saved_query_path(&st, "Ventas".into(), "r".into())
            .await
            .unwrap(),
        dir.path().join("queries").join("Ventas").join("r.sql")
    );
    delete_saved_query(&st, "Ventas".into(), "r".into())
        .await
        .unwrap();
    assert!(list_saved_queries(&st, None).await.is_empty());
    assert_eq!(
        saved_query_path(&st, "Ventas".into(), "r".into()).await,
        Err("not_found".into())
    );
}

#[tokio::test]
async fn palette_finds_saved_queries_and_follows_changes() {
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    save_query(
        &st,
        "Ventas".into(),
        "Reporte mensual".into(),
        "SELECT 1".into(),
        false,
    )
    .await
    .unwrap();
    save_query(
        &st,
        "Ventas".into(),
        "Clientes".into(),
        "SELECT 2".into(),
        false,
    )
    .await
    .unwrap();

    let r = search_objects(&st, "rep".into(), 50).await;
    assert_eq!(
        r.queries,
        [SavedQueryHit {
            database: "Ventas".into(),
            name: "Reporte mensual".into(),
            indices: vec![0, 1, 2],
        }]
    );

    rename_saved_query(
        &st,
        "Ventas".into(),
        "Reporte mensual".into(),
        "Año rep".into(),
    )
    .await
    .unwrap();
    let r = search_objects(&st, "rep".into(), 50).await;
    assert_eq!(r.queries.len(), 1);
    assert_eq!(r.queries[0].name, "Año rep");
    // Code points, not UTF-16 units or bytes.
    assert_eq!(r.queries[0].indices, vec![4, 5, 6]);

    delete_saved_query(&st, "Ventas".into(), "Año rep".into())
        .await
        .unwrap();
    assert!(search_objects(&st, "rep".into(), 50)
        .await
        .queries
        .is_empty());
}

#[tokio::test]
async fn palette_limits_saved_queries_to_20() {
    let dir = TempDir::new().unwrap();
    let st = state(&dir);
    for i in 0..30 {
        save_query(
            &st,
            "Ventas".into(),
            format!("Reporte {i}"),
            "x".into(),
            false,
        )
        .await
        .unwrap();
    }
    let r = search_objects(&st, "rep".into(), 5).await;
    assert_eq!(r.queries.len(), 20);
}

#[tokio::test]
async fn startup_index_is_built_from_disk() {
    let dir = TempDir::new().unwrap();
    QueriesStore::at(dir.path().join("queries"))
        .save("Ventas", "Reporte mensual", "SELECT 1", false)
        .unwrap();
    let st = state(&dir);
    let found = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let r = search_objects(&st, "rep".into(), 50).await;
            if !r.queries.is_empty() {
                return r.queries;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the startup index never loaded");
    assert_eq!(found[0].name, "Reporte mensual");
}

#[test]
fn json_shapes() {
    let q = SavedQuery {
        database: "Ventas".into(),
        name: "q".into(),
        modified_ms: 5,
    };
    assert_eq!(
        serde_json::to_value(&q).unwrap(),
        serde_json::json!({"database": "Ventas", "name": "q", "modifiedMs": 5})
    );
    let h = SavedQueryHit {
        database: "Ventas".into(),
        name: "q".into(),
        indices: vec![0],
    };
    assert_eq!(
        serde_json::to_value(&h).unwrap(),
        serde_json::json!({"database": "Ventas", "name": "q", "indices": [0]})
    );
}
