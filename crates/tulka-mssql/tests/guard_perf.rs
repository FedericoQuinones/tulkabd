//! Guard must not slow down Ctrl+Enter: a 1 MB script is analyzed in under
//! 20 ms in a release build.

use std::time::Instant;

#[test]
#[ignore = "timing budget, run in release"]
fn guard_analyzes_one_megabyte_under_budget() {
    let unit = "UPDATE t SET a = 'x' WHERE id = 1;\nSELECT * FROM t -- c\n";
    let sql = unit.repeat(1_000_000 / unit.len() + 1);
    assert!(sql.len() >= 1_000_000);

    let mut ms: Vec<f64> = (0..5)
        .map(|_| {
            let t = Instant::now();
            let f = tulka_mssql::guard::findings(&sql);
            let e = t.elapsed().as_secs_f64() * 1000.0;
            assert!(f.is_empty());
            e
        })
        .collect();
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = ms[2];
    println!("guard median over 5 runs: {median:.2} ms");
    assert!(median < 20.0, "guard took {median:.2} ms");
}
