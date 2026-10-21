//! Measurement behind the "Copy for AI" separator choice (not a regression
//! test): counts cl100k_base tokens of the same result sets rendered with
//! several candidate formats. cl100k_base is only an approximation of the
//! tokenizers of current models.
//!
//! Run with:
//! `cargo test -p tulka-core --test ai_format_tokens -- --ignored --nocapture`

use tulka_core::{to_ai, Column, Value};

/// Deterministic pseudo-random generator, so the datasets never change.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

type Formatter = Box<dyn Fn(&Dataset) -> String>;

struct Dataset {
    name: &'static str,
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
}

const WORDS: [&str; 24] = [
    "Ferretería",
    "López",
    "Super",
    "Avenida",
    "Distribuidora",
    "Norte",
    "Sur",
    "S.A.",
    "Hnos.",
    "Central",
    "Mayorista",
    "Pedido",
    "entregado",
    "pendiente",
    "cliente",
    "factura",
    "sucursal",
    "Buenos Aires",
    "Córdoba",
    "Rosario",
    "envío urgente",
    "sin cargo",
    "revisar",
    "stock",
];

fn text(rng: &mut Lcg, words: usize) -> String {
    let mut parts = Vec::new();
    for _ in 0..words {
        parts.push(WORDS[rng.below(WORDS.len() as u64) as usize]);
    }
    // Some texts carry commas, as real names and notes do; a few carry `|`.
    let sep = if rng.below(5) == 0 { ", " } else { " " };
    let mut s = parts.join(sep);
    if rng.below(40) == 0 {
        s.push_str(" | extra");
    }
    s
}

fn cell(rng: &mut Lcg, kind: u64, null_pct: u64) -> Value {
    if null_pct > 0 && rng.below(100) < null_pct {
        return Value::Null;
    }
    match kind % 5 {
        0 => Value::Int(rng.below(100000) as i64),
        1 => Value::Decimal(format!("{}.{:02}", rng.below(9_000_000), rng.below(100))),
        2 => {
            let words = 1 + rng.below(3) as usize;
            Value::Text(text(rng, words))
        }
        3 => Value::DateTime(format!(
            "2026-{:02}-{:02} {:02}:{:02}:00",
            1 + rng.below(12),
            1 + rng.below(28),
            rng.below(24),
            rng.below(60)
        )),
        _ => Value::Text(text(rng, 1)),
    }
}

fn dataset(
    name: &'static str,
    ncols: usize,
    nrows: usize,
    null_pct: u64,
    kind: impl Fn(usize) -> u64,
    seed: u64,
) -> Dataset {
    let mut rng = Lcg(seed);
    let columns = (0..ncols)
        .map(|i| Column {
            name: format!("col_{i}"),
            type_name: "x".into(),
        })
        .collect();
    let rows = (0..nrows)
        .map(|_| {
            (0..ncols)
                .map(|c| cell(&mut rng, kind(c), null_pct))
                .collect()
        })
        .collect();
    Dataset {
        name,
        columns,
        rows,
    }
}

fn datasets() -> Vec<Dataset> {
    let mut long = dataset("long texts", 4, 30, 0, |c| c as u64, 4);
    // Replace the text columns with 200-character texts.
    let mut rng = Lcg(99);
    for row in &mut long.rows {
        for v in row.iter_mut() {
            if matches!(v, Value::Text(_)) {
                let mut s = String::new();
                while s.chars().count() < 200 {
                    s.push_str(&text(&mut rng, 3));
                    s.push(' ');
                }
                *v = Value::Text(s.chars().take(200).collect::<String>().trim_end().into());
            }
        }
    }
    vec![
        dataset("wide 20x50", 20, 50, 0, |c| c as u64, 1),
        dataset("narrow 3x200", 3, 200, 0, |c| [0, 2, 1][c], 2),
        dataset("30% nulls 8x100", 8, 100, 30, |c| c as u64, 3),
        long,
    ]
}

/// The raw text of a cell, or `None` for NULL; decimals lose trailing zeros
/// exactly as in `to_ai`.
fn raw(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::Int(i) => Some(i.to_string()),
        Value::Decimal(d) => Some(d.trim_end_matches('0').trim_end_matches('.').to_string()),
        Value::Text(t) | Value::DateTime(t) => Some(t.clone()),
        _ => unreachable!("not generated"),
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Delimited text with one-char separator `sep`, quoting on `sep`, newline
/// and `"`; empty strings as `""`, NULL as nothing.
fn delimited(d: &Dataset, sep: char) -> String {
    let field = |s: Option<String>| match s {
        None => String::new(),
        Some(s) if s.is_empty() || s.contains([sep, '\n', '\r', '"']) => quote(&s),
        Some(s) => s,
    };
    let sep_s = sep.to_string();
    let mut out = d
        .columns
        .iter()
        .map(|c| field(Some(c.name.clone())))
        .collect::<Vec<_>>()
        .join(&sep_s);
    out.push('\n');
    for row in &d.rows {
        out.push_str(
            &row.iter()
                .map(|v| field(raw(v)))
                .collect::<Vec<_>>()
                .join(&sep_s),
        );
        out.push('\n');
    }
    out
}

fn markdown(d: &Dataset) -> String {
    let esc = |s: Option<String>| s.unwrap_or_default().replace('|', "\\|").replace('\n', " ");
    let mut out = format!(
        "| {} |\n|{}\n",
        d.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(" | "),
        "---|".repeat(d.columns.len())
    );
    for row in &d.rows {
        out.push_str(&format!(
            "| {} |\n",
            row.iter()
                .map(|v| esc(raw(v)))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    out
}

/// TOON tabular layout: `name[N]{c1,c2}:` then indented comma-separated rows.
fn toon(d: &Dataset) -> String {
    let field = |s: Option<String>| match s {
        None => "null".to_string(),
        Some(s)
            if s.is_empty()
                || s.contains([',', '"', '\n', '\r', ':'])
                || s != s.trim()
                || matches!(s.as_str(), "null" | "true" | "false") =>
        {
            quote(&s)
        }
        Some(s) => s,
    };
    let mut out = format!(
        "rows[{}]{{{}}}:\n",
        d.rows.len(),
        d.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(",")
    );
    for row in &d.rows {
        out.push_str("  ");
        out.push_str(
            &row.iter()
                .map(|v| field(raw(v)))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

#[test]
#[ignore = "measurement; run manually with --ignored --nocapture"]
fn measure_ai_format_tokens() {
    let bpe = tiktoken_rs::cl100k_base().expect("cl100k_base");
    let count = |s: &str| bpe.encode_ordinary(s).len();

    let formats: [(&str, Formatter); 5] = [
        ("pipe |", Box::new(|d| delimited(d, '|'))),
        ("tab \\t", Box::new(|d| delimited(d, '\t'))),
        ("comma ,", Box::new(|d| delimited(d, ','))),
        ("markdown", Box::new(markdown)),
        ("toon", Box::new(toon)),
    ];

    let data = datasets();
    for d in &data {
        // The measured pipe format must be what `to_ai` really produces
        // (apart from decimals, which `raw` trims the same way).
        assert_eq!(delimited(d, '|'), to_ai(&d.columns, &d.rows), "{}", d.name);
    }

    let mut totals = vec![0usize; formats.len()];
    print!("\n{:<18}", "dataset");
    for (name, _) in &formats {
        print!("{name:>10}");
    }
    println!();
    for d in &data {
        print!("{:<18}", d.name);
        for (i, (_, f)) in formats.iter().enumerate() {
            let n = count(&f(d));
            totals[i] += n;
            print!("{n:>10}");
        }
        println!();
    }
    print!("{:<18}", "TOTAL");
    for t in &totals {
        print!("{t:>10}");
    }
    println!();
    print!("{:<18}", "vs pipe");
    for t in &totals {
        print!("{:>9.1}%", (*t as f64 / totals[0] as f64 - 1.0) * 100.0);
    }
    println!();
}
