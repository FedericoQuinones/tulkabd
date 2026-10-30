//! The metadata tools: list connections and databases, search objects and
//! describe a table.

use std::collections::HashSet;

use tulka_core::{DatabaseScope, DbObject, ObjectKind, TableStructure};
use tulka_store::{CachedDatabase, MetaCache, ObjectQuery};

use super::{format, AiState, ToolText, Visible, AI_SEARCH_LIMIT_DEFAULT, AI_SEARCH_LIMIT_MAX};

/// A cached database that was indexed and is readable.
fn is_indexed(d: &CachedDatabase) -> bool {
    !d.no_access && d.indexed_at.is_some()
}

fn environment_name(v: &Visible) -> String {
    serde_json::to_value(v.conn.environment)
        .ok()
        .and_then(|j| j.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// One row per connection shared with the AI: its label, environment and
/// databases. No server and no user.
pub async fn list_connections(s: &AiState) -> ToolText {
    let visible = match s.visible().await {
        Ok(v) => v,
        Err(e) => return e,
    };
    if visible.is_empty() {
        return ToolText::ok(format::NO_CONNECTIONS);
    }
    let ids: Vec<String> = visible.iter().map(|v| v.conn.id.clone()).collect();
    let cached: Vec<Vec<CachedDatabase>> = s
        .with_cache(move |c| ids.iter().map(|id| c.databases(id)).collect())
        .await
        .unwrap_or_else(|| vec![Vec::new(); visible.len()]);
    let rows = visible
        .iter()
        .zip(cached)
        .map(|(v, dbs)| {
            let databases = match &v.conn.databases {
                DatabaseScope::Selected { names } => format!("selected: {}", names.join(", ")),
                DatabaseScope::All => match dbs.iter().filter(|d| is_indexed(d)).count() {
                    0 => "all (not indexed)".to_string(),
                    n => format!("all ({n} cached)"),
                },
            };
            vec![v.label.clone(), environment_name(v), databases]
        })
        .collect();
    ToolText::ok(format::table(
        &["connection", "environment", "databases"],
        rows,
    ))
}

/// The databases of a connection: the selected ones, or, with all of them,
/// those the server lists now (the cache when it cannot be reached).
pub async fn list_databases(s: &AiState, connection: String) -> ToolText {
    let visible = match s.visible().await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let v = match AiState::find(&visible, &connection) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let DatabaseScope::Selected { names } = &v.conn.databases {
        let rows = names.iter().map(|n| vec![n.clone()]).collect();
        return ToolText::ok(format!(
            "{} › databases (selected)\n{}",
            v.label,
            format::table(&["database"], rows)
        ));
    }

    let id = v.conn.id.clone();
    let cached = s
        .with_cache(move |c| c.databases(&id))
        .await
        .unwrap_or_default();
    let no_access: HashSet<String> = cached
        .iter()
        .filter(|d| d.no_access)
        .map(|d| d.name.to_lowercase())
        .collect();
    let rows = |names: Vec<String>| -> Vec<Vec<String>> {
        names
            .into_iter()
            .map(|n| {
                let access = if no_access.contains(&n.to_lowercase()) {
                    "no access"
                } else {
                    ""
                };
                vec![n, access.to_string()]
            })
            .collect()
    };

    let (driver, params) = match s.prepare(v, None).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let _slot = s.slot().await;
    let live = async {
        let mut session = driver.connect(&params).await?;
        session.list_databases().await
    }
    .await;
    match live {
        Ok(names) => ToolText::ok(format!(
            "{} › databases (all)\n{}",
            v.label,
            format::table(&["database", "access"], rows(names))
        )),
        Err(e) if cached.is_empty() => ToolText::error(format::connect_failed(&v.label, &e)),
        Err(_) => ToolText::ok(format!(
            "{} › databases (all; the server could not be reached, so this is what tulkabd cached)\n{}",
            v.label,
            format::table(
                &["database", "access"],
                rows(cached.iter().map(|d| d.name.clone()).collect())
            )
        )),
    }
}

/// What `search_objects` looks for.
#[derive(Clone, Debug, Default)]
pub struct SearchRequest {
    pub query: String,
    pub connection: Option<String>,
    pub database: Option<String>,
    pub kind: Option<ObjectKind>,
    pub in_columns: bool,
    pub limit: Option<u32>,
}

/// One match of a search, ready to print.
struct Hit {
    connection: String,
    database: String,
    kind: ObjectKind,
    object: String,
    column: Option<String>,
}

fn kind_name(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Table => "table",
        ObjectKind::View => "view",
        ObjectKind::Procedure => "procedure",
    }
}

/// Finds tables, views and procedures (or columns) whose name contains every
/// word of the query, among the connections shared with the AI. It reads the
/// cache the app keeps, except for one named database that is not in it,
/// which is read from the server.
pub async fn search_objects(s: &AiState, req: SearchRequest) -> ToolText {
    let terms: Vec<String> = req.query.split_whitespace().map(str::to_string).collect();
    if terms.is_empty() {
        return ToolText::error("Give at least one word to search for.");
    }
    let limit = req
        .limit
        .unwrap_or(AI_SEARCH_LIMIT_DEFAULT)
        .clamp(1, AI_SEARCH_LIMIT_MAX) as usize;
    let visible = match s.visible().await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let scope: Vec<&Visible> = match &req.connection {
        Some(name) => match AiState::find(&visible, name) {
            Ok(v) => vec![v],
            Err(e) => return e,
        },
        None => visible.iter().collect(),
    };
    if scope.is_empty() {
        return ToolText::ok(format::NO_CONNECTIONS);
    }
    let mut database = req.database.clone().filter(|d| !d.trim().is_empty());
    if let (Some(wanted), [only]) = (&database, scope.as_slice()) {
        match only.database(Some(wanted)) {
            Ok(canonical) => database = canonical,
            Err(e) => return e,
        }
    }

    let ids: Vec<String> = scope.iter().map(|v| v.conn.id.clone()).collect();
    // A connection that selects databases is searched in those alone, even
    // if the cache still holds others from when it exposed all of them.
    let only_databases: Vec<(String, Vec<String>)> = scope
        .iter()
        .filter_map(|v| match &v.conn.databases {
            DatabaseScope::Selected { names } => Some((v.conn.id.clone(), names.clone())),
            DatabaseScope::All => None,
        })
        .collect();
    let cached: Vec<Vec<CachedDatabase>> = {
        let ids = ids.clone();
        s.with_cache(move |c| ids.iter().map(|id| c.databases(id)).collect())
            .await
            .unwrap_or_else(|| vec![Vec::new(); scope.len()])
    };
    let label_of = |id: &str| -> String {
        scope
            .iter()
            .find(|v| v.conn.id == id)
            .map(|v| v.label.clone())
            .unwrap_or_default()
    };

    // One named database that the app has not indexed: read it live.
    let live_database = match (scope.as_slice(), &database) {
        ([_], Some(db))
            if !cached[0]
                .iter()
                .any(|d| is_indexed(d) && d.name.eq_ignore_ascii_case(db)) =>
        {
            Some(db.clone())
        }
        _ => None,
    };

    let (hits, total, capped) = if let Some(db) = live_database {
        match live_search(s, scope[0], &db, &terms, &req).await {
            Ok(mut hits) => {
                let total = hits.len();
                hits.truncate(limit);
                (hits, total, false)
            }
            Err(e) => return e,
        }
    } else {
        let (kind, in_columns, db) = (req.kind, req.in_columns, database.clone());
        let found = s
            .with_cache(move |c| {
                c.find_objects_in(
                    &ObjectQuery {
                        connection_ids: &ids,
                        database: db.as_deref(),
                        terms: &terms,
                        kind,
                        in_columns,
                        limit,
                    },
                    &only_databases,
                )
            })
            .await
            .unwrap_or_default();
        let hits = found
            .hits
            .into_iter()
            .map(|h| Hit {
                connection: label_of(&h.connection_id),
                database: h.database,
                kind: h.object.kind,
                object: format!("{}.{}", h.object.schema, h.object.name),
                column: h.column,
            })
            .collect();
        (hits, found.total, found.capped)
    };

    let shown = hits.len();
    let rows = hits
        .into_iter()
        .map(|h| {
            let mut row = vec![
                h.connection,
                h.database,
                kind_name(h.kind).to_string(),
                h.object,
            ];
            if req.in_columns {
                row.push(h.column.unwrap_or_default());
            }
            row
        })
        .collect();
    let headers: &[&str] = if req.in_columns {
        &["connection", "database", "kind", "object", "column"]
    } else {
        &["connection", "database", "kind", "object"]
    };
    let mut text = format::table(headers, rows);
    text.push_str(&format!(
        "{shown} of {total}{} matches",
        if capped { "+" } else { "" }
    ));
    let unindexed: Vec<&str> = scope
        .iter()
        .zip(&cached)
        .filter(|(_, dbs)| !dbs.iter().any(is_indexed))
        .map(|(v, _)| v.label.as_str())
        .collect();
    if !unindexed.is_empty() && req.database.is_none() {
        text.push_str(&format!(
            "\nNot indexed: {} (open it once in tulkabd to index it).",
            unindexed.join(", ")
        ));
    }
    ToolText::ok(text)
}

/// Searches one database by reading its objects from the server.
async fn live_search(
    s: &AiState,
    v: &Visible,
    database: &str,
    terms: &[String],
    req: &SearchRequest,
) -> Result<Vec<Hit>, ToolText> {
    let (driver, params) = s.prepare(v, Some(database.to_string())).await?;
    let _slot = s.slot().await;
    let snapshot = async {
        let mut session = driver.connect(&params).await?;
        session.snapshot(database).await
    }
    .await
    .map_err(|e| ToolText::error(format::connect_failed(&v.label, &e)))?;

    let lower: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
    let all_in = |text: &str| {
        let text = text.to_lowercase();
        lower.iter().all(|t| text.contains(t.as_str()))
    };
    let mut hits = Vec::new();
    for (i, object) in snapshot.objects.iter().enumerate() {
        if req.kind.is_some_and(|k| k != object.kind) {
            continue;
        }
        let hit = |column: Option<String>| Hit {
            connection: v.label.clone(),
            database: database.to_string(),
            kind: object.kind,
            object: format!("{}.{}", object.schema, object.name),
            column,
        };
        if req.in_columns {
            let columns = snapshot.columns.iter().find(|(idx, _)| *idx == i);
            for c in columns.map(|(_, cols)| cols.as_slice()).unwrap_or_default() {
                if all_in(&c.name) {
                    hits.push(hit(Some(c.name.clone())));
                }
            }
        } else if all_in(&format!("{}.{}", object.schema, object.name)) {
            hits.push(hit(None));
        }
    }
    let whole = lower.join(" ");
    hits.sort_by_cached_key(|h| {
        let name = h
            .column
            .clone()
            .unwrap_or_else(|| h.object.rsplit('.').next().unwrap_or_default().to_string())
            .to_lowercase();
        let rank = if name == whole {
            0
        } else if name.starts_with(&lower[0]) {
            1
        } else {
            2
        };
        (rank, name.chars().count(), name, h.object.clone())
    });
    Ok(hits)
}

/// Columns, foreign keys and indexes of a table or view. `table` is
/// `schema.name`, `[schema].[name]` or `name`; without a schema, the cache
/// tells which one holds it (`dbo` if it is not there).
pub async fn describe_table(
    s: &AiState,
    connection: String,
    database: Option<String>,
    table: String,
) -> ToolText {
    let (schema, name) = match parse_table_name(&table) {
        Some(parsed) => parsed,
        None => {
            return ToolText::error(format!(
                "\"{}\" is not a table name. Use schema.name or just name.",
                table.trim()
            ))
        }
    };
    let visible = match s.visible().await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let v = match AiState::find(&visible, &connection) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let database = match v.database(database.as_deref()) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let (driver, params) = match s.prepare(v, database.clone()).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let _slot = s.slot().await;
    let mut session = match driver.connect(&params).await {
        Ok(session) => session,
        Err(e) => return ToolText::error(format::connect_failed(&v.label, &e)),
    };
    let database = match database {
        Some(db) => db,
        None => match session.current_database().await {
            Ok(db) => db,
            Err(e) => return ToolText::error(format::driver_failure(&e)),
        },
    };

    let (schema, name) = match resolve_schema(s, v, &database, schema, name).await {
        Ok(resolved) => resolved,
        Err(e) => return e,
    };
    let structure = match session.structure(&database, &schema, &name).await {
        Ok(structure) => structure,
        Err(e) => return ToolText::error(format::driver_failure(&e)),
    };
    if structure.columns.is_empty() {
        return ToolText::error(format!(
            "No table or view named {schema}.{name} in {database} on {}. Use search_objects to find it.",
            v.label
        ));
    }
    ToolText::ok(format!(
        "{} › {database} › {schema}.{name}\n{}",
        v.label,
        structure_text(&structure)
    ))
}

/// The schema of `name`: the one given, else the only one that has it in the
/// cache, else `dbo`. Several schemas with that name is an error.
async fn resolve_schema(
    s: &AiState,
    v: &Visible,
    database: &str,
    schema: Option<String>,
    name: String,
) -> Result<(String, String), ToolText> {
    let (id, db, wanted) = (v.conn.id.clone(), database.to_string(), name.to_lowercase());
    let schema_filter = schema.as_ref().map(|s| s.to_lowercase());
    let found: Vec<DbObject> = s
        .with_cache(move |c: &MetaCache| {
            [ObjectKind::Table, ObjectKind::View]
                .into_iter()
                .flat_map(|kind| c.objects(&id, &db, kind))
                .filter(|o| o.name.to_lowercase() == wanted)
                .filter(|o| {
                    schema_filter
                        .as_ref()
                        .is_none_or(|f| o.schema.to_lowercase() == *f)
                })
                .collect()
        })
        .await
        .unwrap_or_default();
    match found.as_slice() {
        [] => Ok((schema.unwrap_or_else(|| "dbo".to_string()), name)),
        [one] => Ok((one.schema.clone(), one.name.clone())),
        several => Err(ToolText::error(format!(
            "\"{name}\" exists in several schemas of {database}: {}. Use schema.name.",
            several
                .iter()
                .map(|o| format!("{}.{}", o.schema, o.name))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn structure_text(structure: &TableStructure) -> String {
    let yes_no = |b: bool| if b { "yes" } else { "no" }.to_string();
    let columns = structure
        .columns
        .iter()
        .map(|c| {
            vec![
                c.name.clone(),
                c.type_name.clone(),
                yes_no(c.nullable),
                if c.primary_key {
                    "PK".into()
                } else {
                    String::new()
                },
                c.default.clone().unwrap_or_default(),
            ]
        })
        .collect();
    let mut out = format!(
        "Columns:\n{}",
        format::table(&["column", "type", "nullable", "key", "default"], columns)
    );
    if structure.foreign_keys.is_empty() {
        out.push_str("Foreign keys: none\n");
    } else {
        let rows = structure
            .foreign_keys
            .iter()
            .map(|f| {
                vec![
                    f.name.clone(),
                    f.columns.join(", "),
                    format!(
                        "{}.{} ({})",
                        f.ref_schema,
                        f.ref_table,
                        f.ref_columns.join(", ")
                    ),
                ]
            })
            .collect();
        out.push_str(&format!(
            "Foreign keys:\n{}",
            format::table(&["name", "columns", "references"], rows)
        ));
    }
    if structure.indexes.is_empty() {
        out.push_str("Indexes: none\n");
    } else {
        let rows = structure
            .indexes
            .iter()
            .map(|i| {
                vec![
                    i.name.clone(),
                    i.columns.join(", "),
                    yes_no(i.unique),
                    yes_no(i.clustered),
                    yes_no(i.primary_key),
                ]
            })
            .collect();
        out.push_str(&format!(
            "Indexes:\n{}",
            format::table(&["name", "columns", "unique", "clustered", "primary"], rows)
        ));
    }
    out
}

/// Splits `schema.name`, `[schema].[name]` or `name` into its parts, without
/// the brackets. Anything else is `None`.
fn parse_table_name(text: &str) -> Option<(Option<String>, String)> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_brackets = false;
    let mut chars = text.trim().chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' if !in_brackets && current.trim().is_empty() => {
                in_brackets = true;
                current.clear();
            }
            ']' if in_brackets => {
                if chars.peek() == Some(&']') {
                    chars.next();
                    current.push(']');
                } else {
                    in_brackets = false;
                }
            }
            '.' if !in_brackets => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    if in_brackets {
        return None;
    }
    parts.push(current);
    let parts: Vec<String> = parts.into_iter().map(|p| p.trim().to_string()).collect();
    if parts.iter().any(String::is_empty) {
        return None;
    }
    match parts.as_slice() {
        [name] => Some((None, name.clone())),
        [schema, name] => Some((Some(schema.clone()), name.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_table_name;

    #[test]
    fn table_names_are_parsed() {
        let some = |s: Option<&str>, n: &str| Some((s.map(str::to_string), n.to_string()));
        assert_eq!(parse_table_name("clientes"), some(None, "clientes"));
        assert_eq!(
            parse_table_name(" dbo.clientes "),
            some(Some("dbo"), "clientes")
        );
        assert_eq!(
            parse_table_name("[sales].[order lines]"),
            some(Some("sales"), "order lines")
        );
        assert_eq!(parse_table_name("[a.b]"), some(None, "a.b"));
        assert_eq!(parse_table_name("[a]]b]"), some(None, "a]b"));
        assert_eq!(parse_table_name("db.dbo.t"), None);
        assert_eq!(parse_table_name("dbo."), None);
        assert_eq!(parse_table_name("[open"), None);
        assert_eq!(parse_table_name(""), None);
    }
}
