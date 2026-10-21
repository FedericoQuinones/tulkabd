//! Database-agnostic core of tulkabd: the value model, the execution events
//! streamed to the UI and the driver interface every database driver
//! implements.

pub mod driver;
pub mod error;
pub mod value;

pub use driver::{
    CancelHandle, Column, ConnectParams, Driver, ExecEvent, MessageKind, ServerInfo, Session,
    APPLICATION_NAME,
};
pub use error::DriverError;
pub use value::Value;

#[cfg(test)]
mod tests {
    //! Pins the JSON shapes the UI relies on.

    use super::*;
    use serde_json::json;

    fn to_json<T: serde::Serialize>(v: &T) -> serde_json::Value {
        serde_json::to_value(v).unwrap()
    }

    #[test]
    fn value_json_shape() {
        assert_eq!(to_json(&Value::Null), json!({"kind": "null"}));
        assert_eq!(to_json(&Value::Int(7)), json!({"kind": "int", "value": 7}));
        assert_eq!(
            to_json(&Value::Decimal("8912450.50".into())),
            json!({"kind": "decimal", "value": "8912450.50"})
        );
        assert_eq!(
            to_json(&Value::DateTimeOffset("2026-10-20 14:03:00 -03:00".into())),
            json!({"kind": "dateTimeOffset", "value": "2026-10-20 14:03:00 -03:00"})
        );
        assert_eq!(
            to_json(&Value::Bytes(vec![10, 11])),
            json!({"kind": "bytes", "value": [10, 11]})
        );
    }

    #[test]
    fn value_round_trips() {
        let v = Value::Decimal("12.5000".into());
        let back: Value = serde_json::from_value(to_json(&v)).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn exec_event_json_shape() {
        let ev = ExecEvent::ResultStart {
            index: 0,
            columns: vec![Column {
                name: "a".into(),
                type_name: "int".into(),
            }],
        };
        assert_eq!(
            to_json(&ev),
            json!({"kind": "resultStart", "index": 0, "columns": [{"name": "a", "typeName": "int"}]})
        );

        let ev = ExecEvent::Rows {
            index: 1,
            rows: vec![vec![Value::Null, Value::Bool(true)]],
        };
        assert_eq!(
            to_json(&ev),
            json!({"kind": "rows", "index": 1, "rows": [[{"kind": "null"}, {"kind": "bool", "value": true}]]})
        );

        // The internal tag is `kind`, so the message severity is serialized
        // as `messageKind`.
        let ev = ExecEvent::Message {
            kind: MessageKind::Info,
            text: "hola".into(),
            line: Some(3),
        };
        assert_eq!(
            to_json(&ev),
            json!({"kind": "message", "messageKind": "info", "text": "hola", "line": 3})
        );

        assert_eq!(
            to_json(&ExecEvent::RowsAffected { count: 3 }),
            json!({"kind": "rowsAffected", "count": 3})
        );
        assert_eq!(
            to_json(&ExecEvent::BatchDone { batch: 0 }),
            json!({"kind": "batchDone", "batch": 0})
        );
        assert_eq!(
            to_json(&ExecEvent::Disconnected {
                reason: "gone".into()
            }),
            json!({"kind": "disconnected", "reason": "gone"})
        );
    }

    #[test]
    fn driver_error_json_shape() {
        assert_eq!(
            to_json(&DriverError::AuthFailed("nope".into())),
            json!({"kind": "authFailed", "value": "nope"})
        );
        assert_eq!(
            to_json(&DriverError::Query {
                message: "Divide by zero error encountered.".into(),
                line: Some(4)
            }),
            json!({"kind": "query", "value": {"message": "Divide by zero error encountered.", "line": 4}})
        );
        assert_eq!(
            to_json(&DriverError::Cancelled),
            json!({"kind": "cancelled"})
        );
    }

    #[test]
    fn connect_params_json_shape_and_defaults() {
        let p = ConnectParams::default();
        assert_eq!(p.application_name, "tulkabd");
        assert_eq!(p.port, 1433);
        assert_eq!(
            to_json(&p),
            json!({
                "server": "",
                "port": 1433,
                "user": "",
                "password": "",
                "database": null,
                "encrypt": true,
                "trustServerCertificate": false,
                "connectTimeoutS": 15,
                "applicationName": "tulkabd"
            })
        );

        // The UI does not need to send the application name.
        let p: ConnectParams = serde_json::from_value(json!({
            "server": "db", "port": 1433, "user": "sa", "password": "x",
            "database": "master", "encrypt": true,
            "trustServerCertificate": true, "connectTimeoutS": 5
        }))
        .unwrap();
        assert_eq!(p.application_name, "tulkabd");
        assert_eq!(p.database.as_deref(), Some("master"));
    }

    #[test]
    fn connect_params_debug_hides_password() {
        let p = ConnectParams {
            password: "s3cret".into(),
            ..ConnectParams::default()
        };
        assert!(!format!("{p:?}").contains("s3cret"));
    }
}
