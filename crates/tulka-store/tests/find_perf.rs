//! Performance budget for `MetaCache::find_objects`.
//! Run with `cargo test -p tulka-store --release -- --ignored find`.

use std::time::{Duration, Instant};

use tulka_core::{DatabaseSnapshot, DbObject, ObjectKind};
use tulka_store::{MetaCache, ObjectQuery};

#[test]
#[ignore = "performance budget; run with --release"]
fn find_objects_750k_under_budget() {
    let dir = tempfile::tempdir().unwrap();
    let cache = MetaCache::open(&dir.path().join("tulkabd.db"));
    let connections: Vec<String> = (0..10).map(|c| format!("conn-{c}")).collect();
    for conn in &connections {
        for db in 0..150 {
            let objects = (0..500)
                .map(|i| DbObject {
                    schema: if i % 7 == 0 { "ventas" } else { "dbo" }.into(),
                    // A tenth of the names contain the searched word.
                    name: if i % 10 == 3 {
                        format!("Pedido{db}_{i}")
                    } else {
                        format!("Tabla{db}_{i}")
                    },
                    kind: ObjectKind::Table,
                })
                .collect();
            let snap = DatabaseSnapshot {
                objects,
                columns: vec![],
            };
            cache
                .replace_database(conn, &format!("db{db}"), &snap, 1)
                .unwrap();
        }
    }
    assert_eq!(cache.all_objects().len(), 750_000);

    // A common word hits the 5000-match cap early; a rare one scans every row.
    for word in ["pedido", "pedido149_493"] {
        let terms = vec![word.to_string()];
        let query = ObjectQuery {
            connection_ids: &connections,
            database: None,
            terms: &terms,
            kind: None,
            in_columns: false,
            limit: 50,
        };
        let mut times: Vec<Duration> = (0..5)
            .map(|_| {
                let start = Instant::now();
                let found = cache.find_objects(&query);
                let elapsed = start.elapsed();
                assert!(found.total > 0);
                elapsed
            })
            .collect();
        times.sort();
        let median = times[times.len() / 2];
        eprintln!("find_objects({word}) over 750000 objects: median {median:?}");
        assert!(
            median < Duration::from_millis(500),
            "{word}: median {median:?}"
        );
    }
}
