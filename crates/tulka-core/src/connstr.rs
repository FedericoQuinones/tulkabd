//! Parsing and formatting SQL Server connection strings.
//!
//! [`parse_connection_string`] reads the ADO.NET / ODBC key-value form
//! (`Server=…;Database=…`), JDBC URLs (`jdbc:sqlserver://host;databaseName=…`)
//! and URIs (`sqlserver://user:pass@host:port/db?encrypt=true`).
//! [`format_connection_string`] writes the ADO.NET form.
//!
//! Errors never carry the pasted text or any of its values: at most the
//! name of the offending key.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::{ConnectionConfig, DatabaseScope};

/// The fields a connection string provides. `None` means the string did not
/// say.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ParsedConnection {
    /// The host, with `\instance` when one is named; `tcp:` is removed.
    pub server: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
    pub encrypt: Option<bool>,
    pub trust_server_certificate: Option<bool>,
    pub connect_timeout_s: Option<u32>,
    /// Keys that were not understood, as written.
    pub ignored_keys: Vec<String>,
}

impl fmt::Debug for ParsedConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParsedConnection")
            .field("server", &self.server)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("database", &self.database)
            .field("encrypt", &self.encrypt)
            .field("trust_server_certificate", &self.trust_server_certificate)
            .field("connect_timeout_s", &self.connect_timeout_s)
            .field("ignored_keys", &self.ignored_keys)
            .finish()
    }
}

/// Why a connection string was rejected. The `Display` strings are the
/// codes the UI translates (`connection.connstr.errors.<code>`).
#[derive(Error, Clone, Debug, PartialEq, Eq)]
pub enum ConnStrError {
    #[error("empty")]
    Empty,
    #[error("no_server")]
    NoServer,
    #[error("bad_port")]
    BadPort,
    /// A value that could not be read; carries only the key name.
    #[error("bad_value:{0}")]
    BadValue(String),
    /// Windows / Entra authentication; only SQL logins are supported.
    #[error("unsupported_auth")]
    UnsupportedAuth,
    #[error("unrecognized")]
    Unrecognized,
}

/// Parses a connection string in ADO.NET, ODBC, JDBC or URI form.
pub fn parse_connection_string(text: &str) -> Result<ParsedConnection, ConnStrError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ConnStrError::Empty);
    }
    let mut b = Builder::default();
    if let Some(rest) = strip_prefix_ci(text, "jdbc:sqlserver://") {
        b.jdbc(rest)?;
    } else if let Some(rest) =
        strip_prefix_ci(text, "sqlserver://").or_else(|| strip_prefix_ci(text, "mssql://"))
    {
        b.uri(rest)?;
    } else {
        for (key, value) in pairs(text)? {
            b.apply(key, value)?;
        }
    }
    b.finish()
}

/// The ADO.NET connection string of `conn`, with `password` when given.
/// `Database=` is written only when the connection selects exactly one
/// database.
pub fn format_connection_string(conn: &ConnectionConfig, password: Option<&str>) -> String {
    let server = conn.server.trim();
    // A named instance on the default port is looked up by the client.
    let server = if server.contains('\\') && conn.port == 1433 {
        server.to_string()
    } else {
        format!("{server},{}", conn.port)
    };
    let mut parts = vec![("Server", server)];
    if let DatabaseScope::Selected { names } = &conn.databases {
        if let [one] = names.as_slice() {
            parts.push(("Database", one.clone()));
        }
    }
    if !conn.user.is_empty() {
        parts.push(("User ID", conn.user.clone()));
    }
    if let Some(password) = password {
        parts.push(("Password", password.to_string()));
    }
    let flag = |b: bool| if b { "True" } else { "False" }.to_string();
    parts.push(("Encrypt", flag(conn.encrypt)));
    parts.push((
        "TrustServerCertificate",
        flag(conn.trust_server_certificate),
    ));
    parts.push(("Connect Timeout", conn.connect_timeout_s.to_string()));
    parts
        .iter()
        .map(|(key, value)| format!("{key}={}", quote(value)))
        .collect::<Vec<_>>()
        .join(";")
}

/// `value`, between double quotes (doubled inside) when it would not read
/// back as is.
fn quote(value: &str) -> String {
    let needs =
        value.contains([';', '=', '"', '\'']) || value.starts_with('{') || value.trim() != value;
    if needs {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// A key this parser understands, by its canonical name; anything else is
/// [`Key::Other`]. Errors only ever name a key through [`Key::name`], so
/// they never carry text from the paste.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Key {
    Server,
    Port,
    User,
    Password,
    Database,
    Instance,
    Encrypt,
    TrustServerCertificate,
    ConnectTimeout,
    IntegratedSecurity,
    Authentication,
    /// Not understood; kept as written.
    Other(String),
}

impl Key {
    /// Resolves a key as written; case and whitespace are ignored.
    fn resolve(key: &str) -> Key {
        let norm: String = key
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        match norm.as_str() {
            "server" | "datasource" | "address" | "addr" | "networkaddress" | "servername" => {
                Key::Server
            }
            "port" | "portnumber" => Key::Port,
            "userid" | "uid" | "user" | "username" => Key::User,
            "password" | "pwd" => Key::Password,
            "database" | "initialcatalog" | "databasename" => Key::Database,
            "instance" | "instancename" => Key::Instance,
            "encrypt" => Key::Encrypt,
            "trustservercertificate" => Key::TrustServerCertificate,
            "connecttimeout" | "connectiontimeout" | "timeout" | "logintimeout" => {
                Key::ConnectTimeout
            }
            "integratedsecurity" | "trusted_connection" => Key::IntegratedSecurity,
            "authentication" => Key::Authentication,
            _ => Key::Other(key.trim().to_string()),
        }
    }

    /// The canonical name, for errors; `None` for a key not understood.
    fn name(&self) -> Option<&'static str> {
        Some(match self {
            Key::Server => "Server",
            Key::Port => "Port",
            Key::User => "User ID",
            Key::Password => "Password",
            Key::Database => "Database",
            Key::Instance => "Instance",
            Key::Encrypt => "Encrypt",
            Key::TrustServerCertificate => "TrustServerCertificate",
            Key::ConnectTimeout => "Connect Timeout",
            Key::IntegratedSecurity => "Integrated Security",
            Key::Authentication => "Authentication",
            Key::Other(_) => return None,
        })
    }

    /// A value of this key could not be read: `bad_value:<canonical name>`,
    /// or `unrecognized` for a key not understood (its name came from the paste).
    fn bad_value(&self) -> ConnStrError {
        match self.name() {
            Some(name) => ConnStrError::BadValue(name.into()),
            None => ConnStrError::Unrecognized,
        }
    }
}

/// Splits `key=value;key=value` pairs. Values may be quoted with `"…"`,
/// `'…'` or `{…}`, the closing delimiter doubled inside as an escape.
fn pairs(text: &str) -> Result<Vec<(Key, String)>, ConnStrError> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ';');
        if rest.is_empty() {
            return Ok(out);
        }
        let eq = match (rest.find('='), rest.find(';')) {
            (Some(eq), Some(semi)) if eq < semi => eq,
            (Some(eq), None) => eq,
            _ => return Err(ConnStrError::Unrecognized),
        };
        let key = rest[..eq].trim();
        if key.is_empty() {
            return Err(ConnStrError::Unrecognized);
        }
        let key = Key::resolve(key);
        let (value, tail) =
            read_value(rest[eq + 1..].trim_start()).ok_or_else(|| key.bad_value())?;
        out.push((key, value));
        rest = tail;
    }
}

/// Reads one value; returns it and the text after it (empty or starting
/// with `;`). `None` for an unterminated quote or text after the closing one.
fn read_value(s: &str) -> Option<(String, &str)> {
    let close = match s.chars().next() {
        Some('"') => '"',
        Some('\'') => '\'',
        Some('{') => '}',
        _ => {
            let end = s.find(';').unwrap_or(s.len());
            return Some((s[..end].trim().to_string(), &s[end..]));
        }
    };
    // The opening delimiters are all one byte.
    let body = &s[1..];
    let mut value = String::new();
    let mut chars = body.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != close {
            value.push(c);
        } else if chars.peek().map(|&(_, n)| n) == Some(close) {
            value.push(close);
            chars.next();
        } else {
            let tail = body[i + 1..].trim_start();
            return (tail.is_empty() || tail.starts_with(';')).then_some((value, tail));
        }
    }
    None
}

fn parse_port(text: &str) -> Result<u16, ConnStrError> {
    match text.trim().parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(ConnStrError::BadPort),
    }
}

fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" => Some(true),
        "false" | "no" => Some(false),
        _ => None,
    }
}

/// Decodes `%XX` escapes; `None` when one is malformed or the result is not
/// UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `.` and `(local)` name the local machine.
fn local_alias(host: &str) -> String {
    let (base, instance) = match host.find('\\') {
        Some(i) => host.split_at(i),
        None => (host, ""),
    };
    if base == "." || base.eq_ignore_ascii_case("(local)") {
        format!("localhost{instance}")
    } else {
        host.to_string()
    }
}

#[derive(Default)]
struct Builder {
    out: ParsedConnection,
    /// From an `instance` key; added to a server without one.
    instance: Option<String>,
    unsupported_auth: bool,
}

impl Builder {
    /// Applies one `key=value` pair.
    fn apply(&mut self, key: Key, value: String) -> Result<(), ConnStrError> {
        let bad = || key.bad_value();
        match &key {
            Key::Server => self.server(&value)?,
            Key::Port => self.out.port = Some(parse_port(&value)?),
            Key::User => self.out.user = Some(value),
            Key::Password => self.out.password = Some(value),
            Key::Database => self.out.database = Some(value).filter(|d| !d.is_empty()),
            Key::Instance => self.instance = Some(value).filter(|i| !i.is_empty()),
            Key::Encrypt => {
                let v = value.trim().to_ascii_lowercase();
                self.out.encrypt = Some(match v.as_str() {
                    "mandatory" | "strict" => true,
                    "optional" | "disable" => false,
                    _ => parse_bool(&v).ok_or_else(bad)?,
                })
            }
            Key::TrustServerCertificate => {
                self.out.trust_server_certificate = Some(parse_bool(&value).ok_or_else(bad)?)
            }
            Key::ConnectTimeout => {
                self.out.connect_timeout_s = Some(value.trim().parse().map_err(|_| bad())?)
            }
            Key::IntegratedSecurity => {
                if value.trim().eq_ignore_ascii_case("sspi")
                    || parse_bool(&value).ok_or_else(bad)?
                {
                    self.unsupported_auth = true;
                }
            }
            Key::Authentication => {
                let method: String = value
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>()
                    .to_ascii_lowercase();
                if !method.is_empty() && method != "sqlpassword" {
                    self.unsupported_auth = true;
                }
            }
            Key::Other(name) => self.out.ignored_keys.push(name.clone()),
        }
        Ok(())
    }

    /// A `Server` value: `[tcp:]host[\instance][,port]`.
    fn server(&mut self, value: &str) -> Result<(), ConnStrError> {
        let mut v = value.trim();
        if let Some(rest) = strip_prefix_ci(v, "tcp:") {
            v = rest;
        }
        if let Some(i) = v.rfind(',') {
            self.out.port = Some(parse_port(&v[i + 1..])?);
            v = &v[..i];
        }
        self.out.server = local_alias(v.trim());
        Ok(())
    }

    /// `jdbc:sqlserver://[host[\instance][:port]][;key=value...]`, after the
    /// scheme.
    fn jdbc(&mut self, rest: &str) -> Result<(), ConnStrError> {
        let (host, props) = rest.split_once(';').unwrap_or((rest, ""));
        let host = host.trim();
        if !host.is_empty() {
            let host = match host.rsplit_once(':') {
                Some((h, port)) => {
                    self.out.port = Some(parse_port(port)?);
                    h
                }
                None => host,
            };
            self.out.server = local_alias(host.trim());
        }
        for (key, value) in pairs(props)? {
            self.apply(key, value)?;
        }
        Ok(())
    }

    /// `sqlserver://[user[:password]@]host[:port][/database][?key=value&...]`,
    /// after the scheme. Parts are percent-decoded.
    fn uri(&mut self, rest: &str) -> Result<(), ConnStrError> {
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, after) = rest.split_at(end);
        let host_port = match authority.rsplit_once('@') {
            Some((userinfo, host_port)) => {
                let (user, password) = match userinfo.split_once(':') {
                    Some((u, p)) => (u, Some(p)),
                    None => (userinfo, None),
                };
                let decode = |s: &str, key: Key| percent_decode(s).ok_or_else(|| key.bad_value());
                self.out.user = Some(decode(user, Key::User)?);
                if let Some(p) = password {
                    self.out.password = Some(decode(p, Key::Password)?);
                }
                host_port
            }
            None => authority,
        };
        let (host, port) = match host_port.strip_prefix('[') {
            // IPv6 literal: [::1]:1433
            Some(v6) => {
                let (h, tail) = v6.split_once(']').ok_or_else(|| Key::Server.bad_value())?;
                (h, tail.strip_prefix(':'))
            }
            None => match host_port.rsplit_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (host_port, None),
            },
        };
        if let Some(port) = port {
            self.out.port = Some(parse_port(port)?);
        }
        let host = percent_decode(host).ok_or_else(|| Key::Server.bad_value())?;
        self.out.server = local_alias(host.trim());

        let (path, query) = after.split_once('?').unwrap_or((after, ""));
        let database = percent_decode(path.trim_start_matches('/'))
            .ok_or_else(|| Key::Database.bad_value())?;
        if !database.is_empty() {
            self.out.database = Some(database);
        }
        for param in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = param.split_once('=').unwrap_or((param, ""));
            let key = Key::resolve(&percent_decode(key).ok_or(ConnStrError::Unrecognized)?);
            match percent_decode(value) {
                Some(value) => self.apply(key, value)?,
                // A parameter this parser does not use is dropped, unnamed.
                None if key.name().is_none() => {}
                None => return Err(key.bad_value()),
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<ParsedConnection, ConnStrError> {
        if self.unsupported_auth {
            return Err(ConnStrError::UnsupportedAuth);
        }
        if self.out.server.is_empty() {
            return Err(ConnStrError::NoServer);
        }
        if let Some(instance) = self.instance {
            if !self.out.server.contains('\\') {
                self.out.server = format!("{}\\{}", self.out.server, instance.trim());
            }
        }
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Environment;

    fn parse(text: &str) -> ParsedConnection {
        parse_connection_string(text).unwrap()
    }

    fn err(text: &str) -> String {
        parse_connection_string(text).unwrap_err().to_string()
    }

    #[test]
    fn ado_net_with_quoted_password() {
        let p = parse(
            r#"Server=tcp:db.empresa.com,1444;Database=Ventas;User ID=fede;Password="a;b""c";Encrypt=True;TrustServerCertificate=True;Connect Timeout=30"#,
        );
        assert_eq!(
            p,
            ParsedConnection {
                server: "db.empresa.com".into(),
                port: Some(1444),
                user: Some("fede".into()),
                password: Some("a;b\"c".into()),
                database: Some("Ventas".into()),
                encrypt: Some(true),
                trust_server_certificate: Some(true),
                connect_timeout_s: Some(30),
                ignored_keys: vec![],
            }
        );
    }

    #[test]
    fn odbc_braces_and_named_instance() {
        let p = parse(r"Data Source=SRV\SQLEXPRESS;Initial Catalog=X;UID=sa;PWD={p}}w}");
        assert_eq!(p.server, r"SRV\SQLEXPRESS");
        assert_eq!(p.port, None);
        assert_eq!(p.database.as_deref(), Some("X"));
        assert_eq!(p.user.as_deref(), Some("sa"));
        assert_eq!(p.password.as_deref(), Some("p}w"));
    }

    #[test]
    fn keys_ignore_case_and_spaces() {
        for key in ["User ID", "userid", "UID", "user id", "USER  ID"] {
            let p = parse(&format!("Server=h;{key}=fede"));
            assert_eq!(p.user.as_deref(), Some("fede"), "{key}");
        }
        let p = parse("server = h ; initial catalog = Db ; password = 'it''s' ");
        assert_eq!(p.server, "h");
        assert_eq!(p.database.as_deref(), Some("Db"));
        assert_eq!(p.password.as_deref(), Some("it's"));
    }

    #[test]
    fn uri_with_percent_decoding() {
        let p = parse(
            "sqlserver://fede:p%40ss@localhost:1433/Ventas?encrypt=false&trustServerCertificate=true",
        );
        assert_eq!(
            p,
            ParsedConnection {
                server: "localhost".into(),
                port: Some(1433),
                user: Some("fede".into()),
                password: Some("p@ss".into()),
                database: Some("Ventas".into()),
                encrypt: Some(false),
                trust_server_certificate: Some(true),
                connect_timeout_s: None,
                ignored_keys: vec![],
            }
        );
    }

    #[test]
    fn uri_instance_as_query_or_escaped_backslash() {
        let p = parse("sqlserver://sa@srv?instance=SQLEXPRESS");
        assert_eq!(p.server, r"srv\SQLEXPRESS");
        assert_eq!(p.port, None);
        let p = parse("sqlserver://srv%5CSQLEXPRESS/Db");
        assert_eq!(p.server, r"srv\SQLEXPRESS");
        assert_eq!(p.database.as_deref(), Some("Db"));
        assert_eq!(err("sqlserver://srv:abc"), "bad_port");
        assert_eq!(err("sqlserver:///Db"), "no_server");
    }

    #[test]
    fn jdbc_url() {
        let p = parse(
            "jdbc:sqlserver://localhost:1433;databaseName=Ventas;user=sa;password=x;encrypt=true",
        );
        assert_eq!(
            p,
            ParsedConnection {
                server: "localhost".into(),
                port: Some(1433),
                user: Some("sa".into()),
                password: Some("x".into()),
                database: Some("Ventas".into()),
                encrypt: Some(true),
                trust_server_certificate: None,
                connect_timeout_s: None,
                ignored_keys: vec![],
            }
        );
        let p = parse(r"jdbc:sqlserver://srv\INST;instanceName=OTHER;password={a;b}");
        assert_eq!(p.server, r"srv\INST");
        assert_eq!(p.password.as_deref(), Some("a;b"));
        let p = parse("jdbc:sqlserver://;serverName=srv;instanceName=INST;portNumber=1500");
        assert_eq!((p.server.as_str(), p.port), (r"srv\INST", Some(1500)));
    }

    #[test]
    fn encrypt_values() {
        for (v, want) in [
            ("true", true),
            ("False", false),
            ("yes", true),
            ("no", false),
            ("Mandatory", true),
            ("optional", false),
            ("Strict", true),
        ] {
            assert_eq!(
                parse(&format!("Server=h;Encrypt={v}")).encrypt,
                Some(want),
                "{v}"
            );
        }
        assert_eq!(err("Server=h;Encrypt=maybe"), "bad_value:Encrypt");
        assert_eq!(
            err("Server=h;Connect Timeout=soon"),
            "bad_value:Connect Timeout"
        );
        assert_eq!(
            err("Server=h;TrustServerCertificate=2"),
            "bad_value:TrustServerCertificate"
        );
    }

    #[test]
    fn integrated_and_other_auth_are_unsupported() {
        assert_eq!(err("Integrated Security=SSPI;Server=x"), "unsupported_auth");
        assert_eq!(err("Server=x;Trusted_Connection=yes"), "unsupported_auth");
        assert_eq!(err("Server=x;Integrated Security=true"), "unsupported_auth");
        assert_eq!(
            err("Server=x;Authentication=Active Directory Password"),
            "unsupported_auth"
        );
        assert_eq!(
            err("jdbc:sqlserver://x;integratedSecurity=true"),
            "unsupported_auth"
        );
        // SQL login spelled out, or integrated security turned off, is fine.
        assert_eq!(parse("Server=x;Authentication=SqlPassword").server, "x");
        assert_eq!(parse("Server=x;Integrated Security=False").server, "x");
    }

    #[test]
    fn errors() {
        assert_eq!(err("Server=x,abc"), "bad_port");
        assert_eq!(err("Server=x,0"), "bad_port");
        assert_eq!(err("Server=x,70000"), "bad_port");
        assert_eq!(err(""), "empty");
        assert_eq!(err("   \n "), "empty");
        assert_eq!(err("hola mundo"), "unrecognized");
        assert_eq!(err("Database=x;User ID=u"), "no_server");
        assert_eq!(err("Server=;User ID=u"), "no_server");
        assert_eq!(err(r#"Server=x;Password="open"#), "bad_value:Password");
        // Errors name keys canonically, never as typed.
        assert_eq!(err("Server=x;ENCRYPT=maybe"), "bad_value:Encrypt");
        assert_eq!(err("Server=x;pwd={open"), "bad_value:Password");
        assert_eq!(
            err("Server=x;connection timeout=soon"),
            "bad_value:Connect Timeout"
        );
        // A malformed value of an unknown key is not named at all.
        assert_eq!(err(r#"Server=x;Secret="open"#), "unrecognized");
    }

    #[test]
    fn an_unknown_uri_parameter_that_does_not_decode_is_dropped() {
        let p = parse("sqlserver://h?mystery=%ZZ&encrypt=false");
        assert_eq!(p.encrypt, Some(false));
        assert_eq!(err("sqlserver://h?encrypt=%ZZ"), "bad_value:Encrypt");
    }

    #[test]
    fn unknown_keys_are_reported_as_ignored() {
        let p = parse("Server=x;Application Name=app;MultipleActiveResultSets=true");
        assert_eq!(
            p.ignored_keys,
            vec!["Application Name", "MultipleActiveResultSets"]
        );
    }

    #[test]
    fn no_error_shows_the_password() {
        const PW: &str = "Zx9-SecretPw";
        let inputs = [
            format!("Server=x,abc;Password={PW}"),
            format!("Password={PW};Encrypt=maybe"),
            format!("Password=\"{PW}"),
            format!("Password={{{PW}"),
            format!("Password={PW};Integrated Security=SSPI"),
            format!("Password={PW}"),
            PW.to_string(),
            format!("{PW};Server=x"),
            format!("Server=x;{PW}"),
            format!("Server=x;Encrypt={PW}"),
            format!("Server=x;Connect Timeout={PW}"),
            format!("Server=x;Authentication={PW}"),
            format!("Server=x,{PW}"),
            format!("sqlserver://u:{PW}@h:{PW}"),
            format!("sqlserver://u:{PW}@/Db"),
            format!("sqlserver://u:%ZZ{PW}@h"),
            format!("jdbc:sqlserver://h:{PW};password={PW}"),
            format!("jdbc:sqlserver://h;password={{{PW}"),
            // Unknown keys are text from the paste too.
            format!("Password=a;{PW}=\"x"),
            format!("Server=h;{PW}={{x"),
            format!("sqlserver://h?{PW}=%ZZ&encrypt=maybe"),
            format!("sqlserver://h?%ZZ{PW}=1"),
            format!("jdbc:sqlserver://h;{PW}='x"),
        ];
        for input in &inputs {
            let e = parse_connection_string(input).unwrap_err();
            assert!(!e.to_string().contains(PW), "{input}: {e}");
            assert!(!format!("{e:?}").contains(PW), "{input}: {e:?}");
        }
        let ok = parse(&format!("Server=x;Password={PW}"));
        assert!(!format!("{ok:?}").contains(PW));
    }

    fn conn(server: &str, port: u16, user: &str) -> ConnectionConfig {
        ConnectionConfig::new("A", server, port, user, Environment::Test)
    }

    #[test]
    fn format_writes_ado_net() {
        let mut c = conn("db.empresa.com", 1444, "fede");
        c.databases = DatabaseScope::Selected {
            names: vec!["Ventas".into()],
        };
        c.trust_server_certificate = true;
        c.connect_timeout_s = 30;
        assert_eq!(
            format_connection_string(&c, None),
            "Server=db.empresa.com,1444;Database=Ventas;User ID=fede;Encrypt=True;\
             TrustServerCertificate=True;Connect Timeout=30"
        );
        assert_eq!(
            format_connection_string(&c, Some("pw")),
            "Server=db.empresa.com,1444;Database=Ventas;User ID=fede;Password=pw;Encrypt=True;\
             TrustServerCertificate=True;Connect Timeout=30"
        );
        // All, or several databases: no Database key.
        c.databases = DatabaseScope::All;
        assert!(!format_connection_string(&c, None).contains("Database="));
        c.databases = DatabaseScope::Selected {
            names: vec!["a".into(), "b".into()],
        };
        assert!(!format_connection_string(&c, None).contains("Database="));
        // A named instance on the default port lets the client look it up.
        let c = conn(r"srv\SQLEXPRESS", 1433, "sa");
        assert!(format_connection_string(&c, None).starts_with(r"Server=srv\SQLEXPRESS;"));
    }

    #[test]
    fn format_then_parse_round_trips() {
        let passwords = [
            "plain",
            "a;b",
            "say \"hi\"",
            "k=v",
            " padded ",
            "it's",
            "{braced}",
            "mix; \"q\" = 'x' ",
            "",
        ];
        for pw in passwords {
            let mut c = conn("db.local", 1500, "us;er \"x\"");
            c.databases = DatabaseScope::Selected {
                names: vec!["My DB;1".into()],
            };
            c.encrypt = false;
            c.connect_timeout_s = 20;
            let text = format_connection_string(&c, Some(pw));
            let p = parse_connection_string(&text).unwrap();
            assert_eq!(
                p,
                ParsedConnection {
                    server: "db.local".into(),
                    port: Some(1500),
                    user: Some("us;er \"x\"".into()),
                    password: Some(pw.into()),
                    database: Some("My DB;1".into()),
                    encrypt: Some(false),
                    trust_server_certificate: Some(false),
                    connect_timeout_s: Some(20),
                    ignored_keys: vec![],
                },
                "{text}"
            );
        }
    }
}
