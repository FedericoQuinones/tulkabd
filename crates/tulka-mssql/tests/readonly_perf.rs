//! The read-only check runs before every query an assistant sends: 1 MB is
//! checked in under 20 ms in a release build.

use std::time::Instant;

#[test]
#[ignore = "timing budget, run in release"]
fn read_only_check_one_megabyte_under_budget() {
    let unit = "SELECT a, b FROM t WHERE id = 1 -- c\n;";
    let sql = unit.repeat(1_000_000 / unit.len() + 1);
    assert!(sql.len() >= 1_000_000);

    let mut ms: Vec<f64> = (0..5)
        .map(|_| {
            let t = Instant::now();
            let v = tulka_mssql::readonly::violation(&sql);
            let e = t.elapsed().as_secs_f64() * 1000.0;
            assert!(v.is_none());
            e
        })
        .collect();
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = ms[2];
    println!("read-only check median over 5 runs: {median:.2} ms");
    assert!(median < 20.0, "read-only check took {median:.2} ms");
}
