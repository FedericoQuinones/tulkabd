//! In-memory fuzzy index over every cached object, for the Ctrl+P palette.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};
use serde::Serialize;
use tulka_core::DbObject;

pub struct SearchEntry {
    pub connection_id: String,
    pub database: String,
    pub object: DbObject,
}

#[derive(Debug, Default, Clone)]
pub struct SearchScope {
    /// `None` searches every connection.
    pub connection_ids: Option<HashSet<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub connection_id: String,
    pub database: String,
    pub object: DbObject,
    pub score: u32,
    /// Character positions in `schema.name` to highlight.
    pub indices: Vec<u32>,
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub total_matches: usize,
    pub databases_matched: usize,
}

struct Entry {
    db_id: u32,
    object: DbObject,
    /// `schema.name`, built once so queries never allocate per entry.
    haystack: Utf32String,
    /// Length of `haystack` in characters, for the tie-break.
    len: u32,
    /// Position of the object name in `haystack`, in characters.
    name_start: u32,
}

/// Added to a match inside the bare name, so a name beats a schema that
/// matches as well as the name would.
const NAME_BONUS: u32 = 16;
/// Added when the name is exactly the query.
const EXACT_BONUS: u32 = 1000;

impl Entry {
    /// The best score of `pattern` over `schema.name` and over the bare name,
    /// and where the text it matched starts in `haystack`.
    fn score(&self, pattern: &Pattern, matcher: &mut Matcher, exact: &str) -> Option<(u32, u32)> {
        let full = pattern
            .score(self.haystack.slice(..), matcher)
            .map(|s| (s, 0));
        let name = pattern
            .score(self.haystack.slice(self.name_start as usize..), matcher)
            .map(|s| {
                // Compared char by char: no allocation per match.
                let exact_name = self
                    .object
                    .name
                    .chars()
                    .flat_map(char::to_lowercase)
                    .eq(exact.chars());
                (
                    s + NAME_BONUS + if exact_name { EXACT_BONUS } else { 0 },
                    self.name_start,
                )
            });
        match (full, name) {
            (Some(f), Some(n)) => Some(if n.0 >= f.0 { n } else { f }),
            (f, n) => f.or(n),
        }
    }
}

struct DbRef {
    conn: u32,
    name: String,
}

/// Flat in-memory index. Connection ids and database names are interned so
/// 750k entries hold two `u32`s instead of duplicated strings.
#[derive(Default)]
pub struct SearchIndex {
    entries: Vec<Entry>,
    conns: Vec<String>,
    conn_ids: HashMap<String, u32>,
    dbs: Vec<DbRef>,
    db_ids: HashMap<(u32, String), u32>,
}

/// Below this many entries a single thread is faster than spawning.
const PARALLEL_MIN: usize = 50_000;
const MAX_THREADS: usize = 8;

struct Partial {
    /// `(score, entry index)`, trimmed to roughly the best `limit`.
    top: Vec<(u32, u32)>,
    total: usize,
    dbs_hit: Vec<bool>,
}

impl SearchIndex {
    pub fn build(entries: Vec<SearchEntry>) -> Self {
        let mut idx = Self::default();
        idx.entries.reserve(entries.len());
        for e in entries {
            let db_id = idx.intern_db(&e.connection_id, &e.database);
            idx.push(db_id, e.object);
        }
        idx
    }

    /// Replaces every object of one database (after a re-index).
    pub fn replace_database(&mut self, connection_id: &str, database: &str, objects: &[DbObject]) {
        let db_id = self.intern_db(connection_id, database);
        self.entries.retain(|e| e.db_id != db_id);
        for o in objects {
            self.push(db_id, o.clone());
        }
    }

    pub fn remove_connection(&mut self, connection_id: &str) {
        let Some(&conn) = self.conn_ids.get(connection_id) else {
            return;
        };
        let dbs = &self.dbs;
        self.entries.retain(|e| dbs[e.db_id as usize].conn != conn);
    }

    fn intern_db(&mut self, connection_id: &str, database: &str) -> u32 {
        let conn = match self.conn_ids.get(connection_id) {
            Some(&c) => c,
            None => {
                let c = self.conns.len() as u32;
                self.conns.push(connection_id.to_string());
                self.conn_ids.insert(connection_id.to_string(), c);
                c
            }
        };
        if let Some(&d) = self.db_ids.get(&(conn, database.to_string())) {
            return d;
        }
        let d = self.dbs.len() as u32;
        self.dbs.push(DbRef {
            conn,
            name: database.to_string(),
        });
        self.db_ids.insert((conn, database.to_string()), d);
        d
    }

    fn push(&mut self, db_id: u32, object: DbObject) {
        let haystack = Utf32String::from(format!("{}.{}", object.schema, object.name));
        let len = haystack.len() as u32;
        let name_start = object.schema.chars().count() as u32 + 1;
        self.entries.push(Entry {
            db_id,
            object,
            haystack,
            len,
            name_start,
        });
    }

    /// Fuzzy search over `schema.name` and over the bare name (whichever scores
    /// better; an exact name match comes first). Ties on score order shorter names
    /// first, then alphabetically (case-insensitive).
    pub fn search(&self, query: &str, scope: &SearchScope, limit: usize) -> SearchResults {
        let Some(pattern) = fuzzy_pattern(query) else {
            return SearchResults::default();
        };
        let exact = query.trim().to_lowercase();
        let allowed: Vec<bool> = self
            .dbs
            .iter()
            .map(|d| match &scope.connection_ids {
                None => true,
                Some(set) => set.contains(&self.conns[d.conn as usize]),
            })
            .collect();

        let threads = if self.entries.len() < PARALLEL_MIN {
            1
        } else {
            std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .min(MAX_THREADS)
        };
        let chunk = self.entries.len().div_ceil(threads).max(1);
        let partials: Vec<Partial> = if threads == 1 {
            vec![self.score_chunk(0, &self.entries, &pattern, &allowed, &exact, limit)]
        } else {
            std::thread::scope(|s| {
                let handles: Vec<_> = self
                    .entries
                    .chunks(chunk)
                    .enumerate()
                    .map(|(i, c)| {
                        let (pattern, allowed, exact) = (&pattern, &allowed, &exact);
                        s.spawn(move || {
                            self.score_chunk(i * chunk, c, pattern, allowed, exact, limit)
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("search worker"))
                    .collect()
            })
        };

        let mut total_matches = 0;
        let mut dbs_hit = vec![false; self.dbs.len()];
        let mut top = Vec::new();
        for p in partials {
            total_matches += p.total;
            for (hit, seen) in p.dbs_hit.iter().zip(dbs_hit.iter_mut()) {
                *seen |= *hit;
            }
            top.extend(p.top);
        }
        top.sort_unstable_by(|a, b| self.cmp(*a, *b));
        top.truncate(limit);

        // Highlight positions only for the winners.
        let mut matcher = Matcher::new(Config::DEFAULT);
        let hits = top
            .into_iter()
            .map(|(score, i)| {
                let e = &self.entries[i as usize];
                let mut indices = Vec::new();
                // Where the winning score came from: the whole text or the name.
                let start = e
                    .score(&pattern, &mut matcher, &exact)
                    .map_or(0, |(_, at)| at);
                pattern.indices(
                    e.haystack.slice(start as usize..),
                    &mut matcher,
                    &mut indices,
                );
                indices.iter_mut().for_each(|i| *i += start);
                indices.sort_unstable();
                indices.dedup();
                let db = &self.dbs[e.db_id as usize];
                SearchHit {
                    connection_id: self.conns[db.conn as usize].clone(),
                    database: db.name.clone(),
                    object: e.object.clone(),
                    score,
                    indices,
                }
            })
            .collect();
        SearchResults {
            hits,
            total_matches,
            databases_matched: dbs_hit.iter().filter(|b| **b).count(),
        }
    }

    fn score_chunk(
        &self,
        base: usize,
        chunk: &[Entry],
        pattern: &Pattern,
        allowed: &[bool],
        exact: &str,
        limit: usize,
    ) -> Partial {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut p = Partial {
            top: Vec::new(),
            total: 0,
            dbs_hit: vec![false; self.dbs.len()],
        };
        let cap = (limit * 8).max(1024);
        for (off, e) in chunk.iter().enumerate() {
            if !allowed[e.db_id as usize] {
                continue;
            }
            let Some((score, _)) = e.score(pattern, &mut matcher, exact) else {
                continue;
            };
            p.total += 1;
            p.dbs_hit[e.db_id as usize] = true;
            p.top.push((score, (base + off) as u32));
            if p.top.len() >= cap {
                self.trim(&mut p.top, limit);
            }
        }
        self.trim(&mut p.top, limit);
        p
    }

    /// Keeps the best `limit` of `top` (unordered).
    fn trim(&self, top: &mut Vec<(u32, u32)>, limit: usize) {
        if top.len() > limit {
            if limit > 0 {
                top.select_nth_unstable_by(limit - 1, |a, b| self.cmp(*a, *b));
            }
            top.truncate(limit);
        }
    }

    /// Best first: higher score, shorter `schema.name`, then alphabetical.
    fn cmp(&self, a: (u32, u32), b: (u32, u32)) -> Ordering {
        b.0.cmp(&a.0).then_with(|| {
            let (x, y) = (&self.entries[a.1 as usize], &self.entries[b.1 as usize]);
            x.len
                .cmp(&y.len)
                .then_with(|| lower_key(x).cmp(lower_key(y)))
                .then(a.1.cmp(&b.1))
        })
    }
}

/// The palette's matcher, shared by objects and saved queries; `None` for a
/// blank query.
fn fuzzy_pattern(query: &str) -> Option<Pattern> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    // Plain fuzzy words: `$`, `^`, `!` and `'` are name characters, not
    // operators (`Sheet1$` is a common Excel-import name).
    Some(Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    ))
}

/// One saved query to index.
pub struct SavedQueryEntry {
    pub database: String,
    pub name: String,
}

/// One match of [`SavedQueryIndex::search`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedQueryMatch {
    pub database: String,
    pub name: String,
    /// Positions in `name` to highlight, in characters (code points).
    pub indices: Vec<u32>,
}

/// Fuzzy index over the names of the saved queries, with the same matcher as
/// [`SearchIndex`]. Small (one entry per `.sql` file), so it is rebuilt
/// whole after every change.
#[derive(Default)]
pub struct SavedQueryIndex {
    entries: Vec<(SavedQueryEntry, Utf32String)>,
}

impl SavedQueryIndex {
    pub fn build(entries: Vec<SavedQueryEntry>) -> Self {
        let entries = entries
            .into_iter()
            .map(|e| {
                let haystack = Utf32String::from(e.name.as_str());
                (e, haystack)
            })
            .collect();
        SavedQueryIndex { entries }
    }

    /// The best `limit` matches of `query` in the names: higher score
    /// first, then shorter name, then alphabetical (case-insensitive), then
    /// by database.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SavedQueryMatch> {
        let Some(pattern) = fuzzy_pattern(query) else {
            return Vec::new();
        };
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut top: Vec<(u32, &SavedQueryEntry, &Utf32String)> = self
            .entries
            .iter()
            .filter_map(|(e, h)| Some((pattern.score(h.slice(..), &mut matcher)?, e, h)))
            .collect();
        top.sort_by(|(sa, a, ha), (sb, b, hb)| {
            sb.cmp(sa)
                .then(ha.len().cmp(&hb.len()))
                .then_with(|| {
                    let lower =
                        |s: &str| s.chars().flat_map(char::to_lowercase).collect::<String>();
                    lower(&a.name).cmp(&lower(&b.name))
                })
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.database.cmp(&b.database))
        });
        top.truncate(limit);
        top.into_iter()
            .map(|(_, e, h)| {
                let mut indices = Vec::new();
                pattern.indices(h.slice(..), &mut matcher, &mut indices);
                indices.sort_unstable();
                indices.dedup();
                SavedQueryMatch {
                    database: e.database.clone(),
                    name: e.name.clone(),
                    indices,
                }
            })
            .collect()
    }
}

fn lower_key(e: &Entry) -> impl Iterator<Item = char> + '_ {
    e.object
        .schema
        .chars()
        .chain(std::iter::once('.'))
        .chain(e.object.name.chars())
        .flat_map(char::to_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tulka_core::ObjectKind;

    fn e(c: &str, d: &str, schema: &str, name: &str, kind: ObjectKind) -> SearchEntry {
        SearchEntry {
            connection_id: c.into(),
            database: d.into(),
            object: DbObject {
                schema: schema.into(),
                name: name.into(),
                kind,
            },
        }
    }

    fn sample() -> SearchIndex {
        SearchIndex::build(vec![
            e("c1", "db1", "dbo", "Pedidos", ObjectKind::Table),
            e("c1", "db1", "dbo", "Clientes", ObjectKind::Table),
            e("c1", "db2", "ventas", "Pedidos_Año", ObjectKind::Table),
            e("c2", "db3", "dbo", "vPedidos", ObjectKind::View),
            e("c2", "db3", "dbo", "PedidosHistorico", ObjectKind::Table),
        ])
    }

    fn names(r: &SearchResults) -> Vec<String> {
        r.hits
            .iter()
            .map(|h| format!("{}.{}", h.object.schema, h.object.name))
            .collect()
    }

    #[test]
    fn a_name_beats_a_schema_with_the_same_text() {
        let idx = SearchIndex::build(vec![
            e("c1", "db", "Sales", "Orders", ObjectKind::Table),
            e("c1", "db", "Sales", "Customers", ObjectKind::Table),
            e("c1", "db", "dbo", "Sales", ObjectKind::Table),
            e("c1", "db", "dbo", "SalesHistory", ObjectKind::Table),
        ]);
        let r = idx.search("sales", &SearchScope::default(), 50);
        let n = names(&r);
        assert_eq!(n[0], "dbo.Sales");
        assert_eq!(n[1], "dbo.SalesHistory");
        assert_eq!(n.len(), 4);
        // The highlight follows the part that matched.
        assert_eq!(r.hits[0].indices, [4, 5, 6, 7, 8]);
        // Querying schema and name still works.
        let r = idx.search("sales.ord", &SearchScope::default(), 50);
        assert_eq!(names(&r)[0], "Sales.Orders");
    }

    #[test]
    fn finds_matches_and_skips_others() {
        let r = sample().search("ped", &SearchScope::default(), 50);
        let n = names(&r);
        assert!(n.contains(&"dbo.Pedidos".to_string()));
        assert!(n.contains(&"ventas.Pedidos_Año".to_string()));
        assert!(!n.contains(&"dbo.Clientes".to_string()));
    }

    #[test]
    fn indices_point_into_schema_dot_name() {
        let r = sample().search("ped", &SearchScope::default(), 50);
        let h = r
            .hits
            .iter()
            .find(|h| h.object.name == "Pedidos" && h.connection_id == "c1")
            .unwrap();
        assert_eq!(h.indices, vec![4, 5, 6]);
    }

    #[test]
    fn finds_the_view() {
        let r = sample().search("vpedidos", &SearchScope::default(), 50);
        assert_eq!(r.hits[0].object.kind, ObjectKind::View);
    }

    #[test]
    fn scope_filters_connections() {
        let scope = SearchScope {
            connection_ids: Some(["c2".to_string()].into()),
        };
        let r = sample().search("ped", &scope, 50);
        assert!(r.hits.iter().all(|h| h.connection_id == "c2"));
        assert_eq!(r.total_matches, 2);
        assert_eq!(r.databases_matched, 1);
    }

    #[test]
    fn totals_count_all_matches_not_just_limit() {
        let r = sample().search("ped", &SearchScope::default(), 1);
        assert_eq!(r.hits.len(), 1);
        assert_eq!(r.total_matches, 4);
        assert_eq!(r.databases_matched, 3);
    }

    #[test]
    fn tie_orders_shorter_name_first() {
        let idx = SearchIndex::build(vec![
            e("c", "d", "dbo", "PedidosHistorico", ObjectKind::Table),
            e("c", "d", "dbo", "Pedidos", ObjectKind::Table),
        ]);
        let r = idx.search("dbo.pedidos", &SearchScope::default(), 10);
        assert_eq!(r.hits[0].object.name, "Pedidos");
    }

    #[test]
    fn tie_same_length_is_alphabetical() {
        let idx = SearchIndex::build(vec![
            e("c", "d", "dbo", "Bbb", ObjectKind::Table),
            e("c", "d", "dbo", "aaa", ObjectKind::Table),
            e("c", "d", "dbo", "Ccc", ObjectKind::Table),
        ]);
        let r = idx.search("dbo", &SearchScope::default(), 10);
        let n: Vec<_> = r.hits.iter().map(|h| h.object.name.as_str()).collect();
        assert_eq!(n, vec!["aaa", "Bbb", "Ccc"]);
    }

    #[test]
    fn empty_or_blank_query_returns_nothing() {
        for q in ["", "   ", "\t"] {
            let r = sample().search(q, &SearchScope::default(), 50);
            assert!(r.hits.is_empty());
            assert_eq!(r.total_matches, 0);
            assert_eq!(r.databases_matched, 0);
        }
    }

    #[test]
    fn operator_characters_match_literally() {
        let idx = SearchIndex::build(vec![
            e("c1", "db1", "dbo", "Sheet1$", ObjectKind::Table),
            e("c1", "db1", "dbo", "!Import", ObjectKind::Table),
            e("c1", "db1", "dbo", "Sheet2", ObjectKind::Table),
        ]);
        let all = SearchScope::default();
        assert_eq!(names(&idx.search("Sheet1$", &all, 50)), ["dbo.Sheet1$"]);
        assert_eq!(names(&idx.search("!imp", &all, 50)), ["dbo.!Import"]);
        // `^` is no anchor: it must appear in the name.
        assert!(idx.search("^sheet", &all, 50).hits.is_empty());
    }

    #[test]
    fn smart_case() {
        let idx = sample();
        assert!(idx.search("Ped", &SearchScope::default(), 5).total_matches > 0);
        assert_eq!(
            idx.search("PED", &SearchScope::default(), 5).total_matches,
            0
        );
    }

    #[test]
    fn unicode_normalization() {
        let r = sample().search("ano", &SearchScope::default(), 5);
        assert!(names(&r).contains(&"ventas.Pedidos_Año".to_string()));
    }

    #[test]
    fn replace_database_swaps_only_that_database() {
        let mut idx = sample();
        let obj = DbObject {
            schema: "dbo".into(),
            name: "Facturas".into(),
            kind: ObjectKind::Table,
        };
        idx.replace_database("c1", "db1", &[obj]);
        let s = SearchScope::default();
        assert_eq!(idx.search("facturas", &s, 5).total_matches, 1);
        let r = idx.search("ped", &s, 50);
        // db1's Pedidos is gone; db2, db3 remain.
        assert_eq!(r.total_matches, 3);
        assert_eq!(idx.search("clientes", &s, 5).total_matches, 0);
    }

    #[test]
    fn remove_connection_drops_its_entries() {
        let mut idx = sample();
        idx.remove_connection("c2");
        let r = idx.search("ped", &SearchScope::default(), 50);
        assert_eq!(r.total_matches, 2);
        assert!(r.hits.iter().all(|h| h.connection_id == "c1"));
    }

    #[test]
    fn serializes_camel_case() {
        let r = sample().search("ped", &SearchScope::default(), 1);
        let v = serde_json::to_value(&r).unwrap();
        assert!(v["totalMatches"].is_number());
        assert!(v["databasesMatched"].is_number());
        assert!(v["hits"][0]["connectionId"].is_string());
    }

    fn saved(entries: &[(&str, &str)]) -> SavedQueryIndex {
        SavedQueryIndex::build(
            entries
                .iter()
                .map(|(d, n)| SavedQueryEntry {
                    database: d.to_string(),
                    name: n.to_string(),
                })
                .collect(),
        )
    }

    #[test]
    fn saved_queries_match_names_with_code_point_indices() {
        let idx = saved(&[
            ("Ventas", "Reporte mensual"),
            ("Ventas", "Clientes"),
            ("Compras", "Año rep"),
        ]);
        let r = idx.search("rep", 20);
        let got: Vec<_> = r
            .iter()
            .map(|m| (m.name.as_str(), m.indices.clone()))
            .collect();
        assert_eq!(
            got,
            [
                ("Año rep", vec![4, 5, 6]),
                ("Reporte mensual", vec![0, 1, 2])
            ]
        );
        assert!(idx.search("  ", 20).is_empty());
        assert!(idx.search("REP", 20).is_empty(), "smart case");
    }

    #[test]
    fn saved_queries_ties_and_limit() {
        let idx = saved(&[("b", "rep"), ("a", "rep"), ("a", "Rep"), ("a", "rep2")]);
        let r = idx.search("rep", 3);
        let got: Vec<_> = r
            .iter()
            .map(|m| (m.database.as_str(), m.name.as_str()))
            .collect();
        // Same score and length: name case-insensitively, then exact name,
        // then database; the longer `rep2` is cut by the limit.
        assert_eq!(got, [("a", "Rep"), ("a", "rep"), ("b", "rep")]);
    }

    fn lcg(s: &mut u64) -> usize {
        *s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*s >> 33) as usize
    }

    #[test]
    #[ignore]
    fn search_budget_750k() {
        const WORDS: &[&str] = &[
            "Pedidos",
            "Clientes",
            "Facturas",
            "Productos",
            "Ventas",
            "Compras",
            "Stock",
            "Usuarios",
            "Detalle",
            "Historico",
            "Cuentas",
            "Pagos",
            "Envios",
            "Proveedores",
            "Almacen",
            "Orden",
            "Items",
            "Config",
            "Log",
            "Audit",
            "Reporte",
            "Movimientos",
            "Precios",
            "Sucursal",
        ];
        const SCHEMAS: &[&str] = &["dbo", "ventas", "rrhh", "stock", "audit"];
        let mut seed = 42u64;
        let mut entries = Vec::with_capacity(750_000);
        for c in 0..10 {
            for d in 0..150 {
                for _ in 0..500 {
                    let w1 = WORDS[lcg(&mut seed) % WORDS.len()];
                    let w2 = WORDS[lcg(&mut seed) % WORDS.len()];
                    let n = lcg(&mut seed) % 1000;
                    let name = match lcg(&mut seed) % 3 {
                        0 => format!("{w1}{w2}"),
                        1 => format!("{w1}_{w2}_{n}"),
                        _ => format!("{}{n}", w1.to_lowercase()),
                    };
                    let kind = match lcg(&mut seed) % 5 {
                        0 => ObjectKind::View,
                        1 => ObjectKind::Procedure,
                        _ => ObjectKind::Table,
                    };
                    entries.push(e(
                        &format!("conn{c}"),
                        &format!("db{d}"),
                        SCHEMAS[lcg(&mut seed) % SCHEMAS.len()],
                        &name,
                        kind,
                    ));
                }
            }
        }
        let t = Instant::now();
        let idx = SearchIndex::build(entries);
        println!("build 750k: {:?}", t.elapsed());
        let scope = SearchScope::default();
        for q in ["ped", "p"] {
            let _ = idx.search(q, &scope, 50); // warm up
            let mut worst = std::time::Duration::ZERO;
            let mut r = SearchResults::default();
            for _ in 0..5 {
                let t = Instant::now();
                r = idx.search(q, &scope, 50);
                worst = worst.max(t.elapsed());
            }
            println!(
                "search {q:?}: worst of 5 = {worst:?}, matches {}, dbs {}",
                r.total_matches, r.databases_matched
            );
            assert_eq!(r.hits.len(), 50);
            assert!(worst.as_millis() < 50, "{q:?} took {worst:?}");
        }
    }
}
