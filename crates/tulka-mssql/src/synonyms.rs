//! Where the base object of a synonym lives and how its name resolves.
//!
//! A part left out of the base name is not an error: a missing schema (`T`
//! or `db..T`) resolves when the synonym is used, first in the schema of the
//! synonym and then in `dbo`. A base object on a linked server, or one that
//! is itself a synonym, is never followed.

use std::collections::HashMap;

use tiberius::Row;

/// A synonym and the parts of its base name, as read by [`meta::synonyms_sql`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynonymBase {
    pub schema: String,
    pub name: String,
    pub server: Option<String>,
    pub database: Option<String>,
    pub base_schema: Option<String>,
    pub base_name: String,
}

/// Where the base object is looked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// In the database being read, in these schemas in this order.
    Local(Vec<String>),
    /// In another database of the server, in these schemas in this order.
    Foreign(String, Vec<String>),
    /// Out of reach: a linked server.
    Unknown,
}

fn part(row: &Row, i: usize) -> Option<String> {
    row.get::<&str, _>(i)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl SynonymBase {
    pub fn from_row(row: &Row) -> Option<Self> {
        Some(SynonymBase {
            schema: part(row, 0)?,
            name: part(row, 1)?,
            server: part(row, 2),
            database: part(row, 3),
            base_schema: part(row, 4),
            base_name: part(row, 5)?,
        })
    }

    /// The schemas the base object can be in, best first.
    pub fn candidates(&self) -> Vec<String> {
        match &self.base_schema {
            Some(schema) => vec![schema.clone()],
            None if self.schema.eq_ignore_ascii_case("dbo") => vec![self.schema.clone()],
            None => vec![self.schema.clone(), "dbo".to_string()],
        }
    }

    /// Where to look, given the database being read.
    pub fn place(&self, current_db: &str) -> Place {
        let candidates = self.candidates();
        if self.server.is_some() {
            return Place::Unknown;
        }
        match &self.database {
            None => Place::Local(candidates),
            Some(db) if db == current_db || db.eq_ignore_ascii_case(current_db) => {
                Place::Local(candidates)
            }
            Some(db) => Place::Foreign(db.clone(), candidates),
        }
    }
}

/// Finds objects by `(schema, name)`: exact spelling first, then ignoring
/// letter case, because the catalog's collation may or may not.
pub struct NameIndex {
    exact: HashMap<(String, String), usize>,
    folded: HashMap<(String, String), usize>,
}

impl NameIndex {
    pub fn new<'a>(items: impl Iterator<Item = (&'a str, &'a str)>) -> Self {
        let mut exact = HashMap::new();
        let mut folded = HashMap::new();
        for (i, (schema, name)) in items.enumerate() {
            exact
                .entry((schema.to_string(), name.to_string()))
                .or_insert(i);
            folded
                .entry((schema.to_lowercase(), name.to_lowercase()))
                .or_insert(i);
        }
        NameIndex { exact, folded }
    }

    /// Position of the first of `schemas` (in order) holding `name`; an exact
    /// match in any of them wins over a case-insensitive one.
    pub fn find(&self, schemas: &[String], name: &str) -> Option<usize> {
        schemas
            .iter()
            .find_map(|s| self.exact.get(&(s.clone(), name.to_string())))
            .or_else(|| {
                schemas
                    .iter()
                    .find_map(|s| self.folded.get(&(s.to_lowercase(), name.to_lowercase())))
            })
            .copied()
    }
}

/// Position of the base object named `name` in `cands` among the objects
/// `index` was built from (`kinds` are their kinds), or `None` when it is
/// missing or is itself a synonym.
pub fn resolve_local(
    index: &NameIndex,
    kinds: &[tulka_core::ObjectKind],
    cands: &[String],
    name: &str,
) -> Option<usize> {
    index
        .find(cands, name)
        .filter(|&i| kinds[i] != tulka_core::ObjectKind::Synonym)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tulka_core::ObjectKind::{Synonym, Table};

    fn syn(schema: &str, db: Option<&str>, base_schema: Option<&str>) -> SynonymBase {
        SynonymBase {
            schema: schema.into(),
            name: "s".into(),
            server: None,
            database: db.map(str::to_string),
            base_schema: base_schema.map(str::to_string),
            base_name: "T".into(),
        }
    }

    #[test]
    fn a_missing_schema_tries_the_synonyms_schema_then_dbo() {
        assert_eq!(syn("ventas", None, None).candidates(), ["ventas", "dbo"]);
        assert_eq!(syn("dbo", None, None).candidates(), ["dbo"]);
        assert_eq!(syn("ventas", None, Some("x")).candidates(), ["x"]);
        // `db..T` leaves the schema out as well.
        assert_eq!(
            syn("ventas", Some("other"), None).place("here"),
            Place::Foreign("other".into(), vec!["ventas".into(), "dbo".into()])
        );
    }

    #[test]
    fn places_by_name_parts() {
        let local = |s: &SynonymBase| s.place("here");
        assert_eq!(
            local(&syn("dbo", None, Some("dbo"))),
            Place::Local(vec!["dbo".into()])
        );
        // Three parts naming the current database are local, exact or not.
        assert!(matches!(
            local(&syn("dbo", Some("here"), Some("dbo"))),
            Place::Local(_)
        ));
        assert!(matches!(
            local(&syn("dbo", Some("HERE"), Some("dbo"))),
            Place::Local(_)
        ));
        assert!(matches!(
            local(&syn("dbo", Some("there"), Some("dbo"))),
            Place::Foreign(db, _) if db == "there"
        ));
        let mut linked = syn("dbo", Some("there"), Some("dbo"));
        linked.server = Some("srv".into());
        assert_eq!(local(&linked), Place::Unknown);
    }

    #[test]
    fn names_match_exactly_before_ignoring_case() {
        let items = [("dbo", "T"), ("DBO", "t")];
        let index = NameIndex::new(items.iter().map(|(s, n)| (*s, *n)));
        let dbo = ["dbo".to_string()];
        assert_eq!(index.find(&dbo, "T"), Some(0));
        assert_eq!(index.find(&["DBO".to_string()], "t"), Some(1));
        // No exact spelling: the first case-insensitive match.
        assert_eq!(index.find(&dbo, "t"), Some(0));
        assert_eq!(index.find(&dbo, "missing"), None);
    }

    #[test]
    fn candidates_are_tried_in_order() {
        let items = [("dbo", "T"), ("ventas", "T")];
        let index = NameIndex::new(items.iter().map(|(s, n)| (*s, *n)));
        let both = ["ventas".to_string(), "dbo".to_string()];
        assert_eq!(index.find(&both, "T"), Some(1));
        let only_dbo = NameIndex::new([("dbo", "Solo")].into_iter());
        assert_eq!(only_dbo.find(&both, "Solo"), Some(0));
    }

    #[test]
    fn a_synonym_of_a_synonym_resolves_to_nothing() {
        let items = [("dbo", "chain"), ("dbo", "T")];
        let index = NameIndex::new(items.iter().map(|(s, n)| (*s, *n)));
        let kinds = [Synonym, Table];
        let dbo = ["dbo".to_string()];
        assert_eq!(resolve_local(&index, &kinds, &dbo, "chain"), None);
        assert_eq!(resolve_local(&index, &kinds, &dbo, "T"), Some(1));
    }
}
