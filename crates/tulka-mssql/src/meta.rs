//! SQL for the object explorer, and the mapping of its rows.
//!
//! Every query reads the catalog of one database through three-part names
//! (`[db].sys.objects`), so the session never switches database. The database
//! name always goes through [`quote_ident`]; every other value is a parameter.

use tiberius::Row;
use tulka_core::{
    quote_ident, ColumnInfo, DbObject, DriverError, ForeignKey, IndexInfo, ObjectKind,
};

/// `[db].` for a three-part name, or nothing for single-database servers
/// (Azure SQL Database), where only the current database can be read.
pub fn catalog_prefix(database: &str, single_database: bool) -> String {
    if single_database {
        String::new()
    } else {
        format!("{}.", quote_ident(database))
    }
}

/// Parameters: `@P1` database name. Returns `HAS_DBACCESS` and the status.
pub const ACCESS_SQL: &str = "SELECT CAST(HAS_DBACCESS(@P1) AS int), \
     CAST(DATABASEPROPERTYEX(@P1, 'Status') AS nvarchar(60))";

/// Whether a result of [`ACCESS_SQL`] allows reading the database.
pub fn access_error(
    database: &str,
    has_access: Option<i32>,
    status: Option<&str>,
) -> Option<DriverError> {
    match (has_access, status) {
        (_, None) => Some(DriverError::NoAccess(format!(
            "the database `{database}` does not exist"
        ))),
        (_, Some(status)) if !status.eq_ignore_ascii_case("ONLINE") => Some(DriverError::NoAccess(
            format!("the database `{database}` is not online ({status})"),
        )),
        (Some(1), _) => None,
        _ => Some(DriverError::NoAccess(format!(
            "no access to the database `{database}`"
        ))),
    }
}

pub fn objects_sql(prefix: &str) -> String {
    format!(
        "SELECT s.name, o.name, o.type \
         FROM {prefix}sys.objects o \
         JOIN {prefix}sys.schemas s ON s.schema_id = o.schema_id \
         WHERE o.type IN ('U', 'V', 'P') AND o.is_ms_shipped = 0 \
         ORDER BY s.name, o.name"
    )
}

pub fn object_from_row(row: &Row) -> Option<DbObject> {
    let kind = match row.get::<&str, _>(2)?.trim() {
        "U" => ObjectKind::Table,
        "V" => ObjectKind::View,
        "P" => ObjectKind::Procedure,
        _ => return None,
    };
    Some(DbObject {
        schema: row.get::<&str, _>(0)?.to_string(),
        name: row.get::<&str, _>(1)?.to_string(),
        kind,
    })
}

/// Columns of tables and views, in object and column order. With `one`, only
/// the object named by `@P1` (name) and `@P2` (schema).
///
/// Result: schema, object, column, type, user-defined, max_length, precision,
/// scale, nullable, primary key, default.
pub fn columns_sql(prefix: &str, one: bool) -> String {
    let filter = if one {
        "AND o.name = @P1 AND s.name = @P2"
    } else {
        ""
    };
    format!(
        "SELECT s.name, o.name, c.name, t.name, CAST(t.is_user_defined AS bit), \
           CAST(c.max_length AS int), CAST(c.precision AS int), CAST(c.scale AS int), \
           c.is_nullable, \
           CAST(CASE WHEN EXISTS (SELECT 1 \
               FROM {prefix}sys.index_columns ic \
               JOIN {prefix}sys.indexes i \
                 ON i.object_id = ic.object_id AND i.index_id = ic.index_id \
               WHERE i.is_primary_key = 1 \
                 AND ic.object_id = c.object_id AND ic.column_id = c.column_id) \
             THEN 1 ELSE 0 END AS bit), \
           dc.definition \
         FROM {prefix}sys.objects o \
         JOIN {prefix}sys.schemas s ON s.schema_id = o.schema_id \
         JOIN {prefix}sys.columns c ON c.object_id = o.object_id \
         JOIN {prefix}sys.types t ON t.user_type_id = c.user_type_id \
         LEFT JOIN {prefix}sys.default_constraints dc \
           ON dc.parent_object_id = c.object_id AND dc.parent_column_id = c.column_id \
         WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0 {filter} \
         ORDER BY s.name, o.name, c.column_id"
    )
}

/// A column row: the owner `(schema, object)` and the column.
pub fn column_from_row(row: &Row) -> Option<((String, String), ColumnInfo)> {
    let int = |i| row.get::<i32, _>(i).unwrap_or(0);
    let type_name = format_type(
        row.get::<&str, _>(3)?,
        row.get::<bool, _>(4).unwrap_or(false),
        int(5),
        int(6),
        int(7),
    );
    Some((
        (
            row.get::<&str, _>(0)?.to_string(),
            row.get::<&str, _>(1)?.to_string(),
        ),
        ColumnInfo {
            name: row.get::<&str, _>(2)?.to_string(),
            type_name,
            nullable: row.get::<bool, _>(8).unwrap_or(true),
            primary_key: row.get::<bool, _>(9).unwrap_or(false),
            default: row.get::<&str, _>(10).map(str::to_string),
        },
    ))
}

/// Formats a column type for display: `nvarchar(50)`, `decimal(18,2)`,
/// `varbinary(max)`, `datetime2(7)`. `max_length` is in bytes, -1 for `max`.
pub fn format_type(
    name: &str,
    user_defined: bool,
    max_length: i32,
    precision: i32,
    scale: i32,
) -> String {
    if user_defined {
        return name.to_string();
    }
    match name {
        "char" | "varchar" | "binary" | "varbinary" | "nchar" | "nvarchar" => {
            if max_length == -1 {
                format!("{name}(max)")
            } else if name.starts_with('n') {
                format!("{name}({})", max_length / 2)
            } else {
                format!("{name}({max_length})")
            }
        }
        "decimal" | "numeric" => format!("{name}({precision},{scale})"),
        "datetime2" | "time" | "datetimeoffset" => format!("{name}({scale})"),
        _ => name.to_string(),
    }
}

/// Parameters: `@P1` table, `@P2` schema. Result: constraint, column, referenced
/// schema, table and column, ordered so a key's columns are adjacent.
pub fn foreign_keys_sql(prefix: &str) -> String {
    format!(
        "SELECT fk.name, pc.name, rs.name, rt.name, rc.name \
         FROM {prefix}sys.foreign_keys fk \
         JOIN {prefix}sys.objects po ON po.object_id = fk.parent_object_id \
         JOIN {prefix}sys.schemas ps ON ps.schema_id = po.schema_id \
         JOIN {prefix}sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
         JOIN {prefix}sys.columns pc \
           ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
         JOIN {prefix}sys.objects rt ON rt.object_id = fkc.referenced_object_id \
         JOIN {prefix}sys.schemas rs ON rs.schema_id = rt.schema_id \
         JOIN {prefix}sys.columns rc \
           ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
         WHERE po.name = @P1 AND ps.name = @P2 \
         ORDER BY fk.name, fkc.constraint_column_id"
    )
}

pub fn foreign_keys_from_rows(rows: &[Row]) -> Vec<ForeignKey> {
    let mut keys: Vec<ForeignKey> = Vec::new();
    for row in rows {
        let (Some(name), Some(column), Some(ref_schema), Some(ref_table), Some(ref_column)) = (
            row.get::<&str, _>(0),
            row.get::<&str, _>(1),
            row.get::<&str, _>(2),
            row.get::<&str, _>(3),
            row.get::<&str, _>(4),
        ) else {
            continue;
        };
        match keys.last_mut().filter(|k| k.name == name) {
            Some(key) => {
                key.columns.push(column.to_string());
                key.ref_columns.push(ref_column.to_string());
            }
            None => keys.push(ForeignKey {
                name: name.to_string(),
                columns: vec![column.to_string()],
                ref_schema: ref_schema.to_string(),
                ref_table: ref_table.to_string(),
                ref_columns: vec![ref_column.to_string()],
            }),
        }
    }
    keys
}

/// Parameters: `@P1` table, `@P2` schema. Result: index, column, unique,
/// primary key, clustered. Heaps and included columns are left out.
pub fn indexes_sql(prefix: &str) -> String {
    format!(
        "SELECT i.name, c.name, i.is_unique, i.is_primary_key, \
           CAST(CASE WHEN i.type = 1 THEN 1 ELSE 0 END AS bit) \
         FROM {prefix}sys.indexes i \
         JOIN {prefix}sys.objects o ON o.object_id = i.object_id \
         JOIN {prefix}sys.schemas s ON s.schema_id = o.schema_id \
         JOIN {prefix}sys.index_columns ic \
           ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
         JOIN {prefix}sys.columns c \
           ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
         WHERE o.name = @P1 AND s.name = @P2 \
           AND i.type > 0 AND i.is_hypothetical = 0 AND ic.is_included_column = 0 \
         ORDER BY i.index_id, ic.key_ordinal, ic.column_id"
    )
}

pub fn indexes_from_rows(rows: &[Row]) -> Vec<IndexInfo> {
    let mut indexes: Vec<IndexInfo> = Vec::new();
    for row in rows {
        let (Some(name), Some(column)) = (row.get::<&str, _>(0), row.get::<&str, _>(1)) else {
            continue;
        };
        match indexes.last_mut().filter(|i| i.name == name) {
            Some(index) => index.columns.push(column.to_string()),
            None => indexes.push(IndexInfo {
                name: name.to_string(),
                columns: vec![column.to_string()],
                unique: row.get::<bool, _>(2).unwrap_or(false),
                primary_key: row.get::<bool, _>(3).unwrap_or(false),
                clustered: row.get::<bool, _>(4).unwrap_or(false),
            }),
        }
    }
    indexes
}

/// Parameters: `@P1` object, `@P2` schema. The definition is `NULL` without
/// the VIEW DEFINITION permission.
pub fn definition_sql(prefix: &str) -> String {
    format!(
        "SELECT m.definition \
         FROM {prefix}sys.sql_modules m \
         JOIN {prefix}sys.objects o ON o.object_id = m.object_id \
         JOIN {prefix}sys.schemas s ON s.schema_id = o.schema_id \
         WHERE o.name = @P1 AND s.name = @P2"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_are_formatted_for_display() {
        assert_eq!(format_type("nvarchar", false, 100, 0, 0), "nvarchar(50)");
        assert_eq!(format_type("nvarchar", false, -1, 0, 0), "nvarchar(max)");
        assert_eq!(format_type("varchar", false, 30, 0, 0), "varchar(30)");
        assert_eq!(format_type("varbinary", false, -1, 0, 0), "varbinary(max)");
        assert_eq!(format_type("decimal", false, 9, 18, 2), "decimal(18,2)");
        assert_eq!(format_type("numeric", false, 5, 10, 0), "numeric(10,0)");
        assert_eq!(format_type("datetime2", false, 8, 27, 7), "datetime2(7)");
        assert_eq!(format_type("int", false, 4, 10, 0), "int");
        assert_eq!(format_type("Dinero", true, 9, 18, 2), "Dinero");
    }

    #[test]
    fn the_database_is_quoted_in_the_prefix() {
        assert_eq!(catalog_prefix("my db]", false), "[my db]]].");
        assert_eq!(catalog_prefix("anything", true), "");
    }

    #[test]
    fn access_needs_an_online_database_and_permission() {
        assert!(access_error("d", Some(1), Some("ONLINE")).is_none());
        for (access, status) in [
            (Some(0), Some("ONLINE")),
            (None, None),
            (Some(0), Some("OFFLINE")),
            (Some(1), Some("RESTORING")),
        ] {
            let e = access_error("d", access, status);
            assert!(matches!(e, Some(DriverError::NoAccess(m)) if m.contains("`d`")));
        }
    }
}
