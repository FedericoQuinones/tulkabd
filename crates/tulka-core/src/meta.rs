//! Metadata types returned by drivers for the object explorer and search.

use serde::{Deserialize, Serialize};

/// Kind of a database object listed in the explorer.
///
/// Serialized as a plain string: `"table"`, `"view"`, `"procedure"`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    Table,
    View,
    Procedure,
}

/// A table, view or procedure.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DbObject {
    pub schema: String,
    pub name: String,
    pub kind: ObjectKind,
}

/// A column of a table or view.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    /// Already formatted for display: `nvarchar(50)`, `decimal(18,2)`,
    /// `nvarchar(max)`.
    pub type_name: String,
    pub nullable: bool,
    pub primary_key: bool,
    pub default: Option<String>,
}

/// A foreign key from a table to another one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

/// An index of a table.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary_key: bool,
    pub clustered: bool,
}

/// Columns, foreign keys and indexes of one table.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TableStructure {
    pub columns: Vec<ColumnInfo>,
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<IndexInfo>,
}

/// Every object of a database plus the columns of its tables and views.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseSnapshot {
    pub objects: Vec<DbObject>,
    /// Columns per object, keyed by the object's index in `objects`.
    pub columns: Vec<(usize, Vec<ColumnInfo>)>,
}

/// Quotes an identifier: `[name]`, doubling any `]`.
pub fn quote_ident(name: &str) -> String {
    format!("[{}]", name.replace(']', "]]"))
}

/// `[schema].[name]`.
pub fn qualified(schema: &str, name: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn quoting() {
        assert_eq!(quote_ident("Order Details"), "[Order Details]");
        assert_eq!(quote_ident("a]b"), "[a]]b]");
        assert_eq!(qualified("dbo", "Pedidos"), "[dbo].[Pedidos]");
    }

    #[test]
    fn db_object_json_shape() {
        let o = DbObject {
            schema: "dbo".into(),
            name: "Pedidos".into(),
            kind: ObjectKind::Table,
        };
        assert_eq!(
            serde_json::to_value(&o).unwrap(),
            json!({"schema": "dbo", "name": "Pedidos", "kind": "table"})
        );
    }

    #[test]
    fn column_info_json_shape() {
        let c = ColumnInfo {
            name: "id".into(),
            type_name: "int".into(),
            nullable: false,
            primary_key: true,
            default: None,
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({"name": "id", "typeName": "int", "nullable": false, "primaryKey": true, "default": null})
        );
    }

    #[test]
    fn server_info_json_shape() {
        let s = crate::ServerInfo {
            version: "v".into(),
            user: "sa".into(),
            single_database: true,
        };
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            json!({"version": "v", "user": "sa", "singleDatabase": true})
        );
    }

    #[test]
    fn no_access_error_json_shape() {
        assert_eq!(
            serde_json::to_value(crate::DriverError::NoAccess("db".into())).unwrap(),
            json!({"kind": "noAccess", "value": "db"})
        );
    }
}
