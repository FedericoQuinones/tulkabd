//! Hides secret literals in SQL text before it is stored in the run log.
//! The statement that is executed is never touched: only the stored copy is.

/// What replaces a secret.
const MASK: &str = "***";

/// How deep dynamic SQL inside string literals is looked into.
const MAX_DEPTH: usize = 4;

/// Words that introduce a secret literal as `WORD = '...'`.
const SECRET_KEYWORDS: &[&str] = &[
    "password",
    "old_password",
    "new_password",
    "mediapassword",
    "secret",
    "key_source",
];

/// System procedures that take a secret: positions (0-based) and parameter
/// names (without the `@`) of the secret arguments.
const SECRET_PROCS: &[(&str, &[usize], &[&str])] = &[
    ("sp_addlogin", &[1], &["passwd"]),
    ("sp_password", &[0, 1], &["old", "new"]),
    ("sp_addlinkedsrvlogin", &[4], &["rmtpassword"]),
    ("sp_setapprole", &[1], &["password"]),
    ("sp_addapprole", &[1], &["password"]),
];

/// Keys of a connection string whose value is a secret.
const CONN_KEYS: &[&str] = &["password", "pwd"];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Space,
    Comment,
    /// A string literal; `start..end` is the content between the quotes.
    Str,
    /// An unquoted word or number.
    Word,
    /// A `[bracketed]` or `"quoted"` identifier; `start..end` is its name.
    Ident,
    Var,
    Punct,
}

struct Tok {
    kind: Kind,
    /// Whole token.
    lo: usize,
    hi: usize,
    /// Content range (strings and identifiers).
    start: usize,
    end: usize,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '#' | '$')
}

fn tok(kind: Kind, lo: usize, hi: usize) -> Tok {
    Tok {
        kind,
        lo,
        hi,
        start: lo,
        end: hi,
    }
}

/// Scans a quoted run starting at `open` (the opening quote), where a doubled
/// closer is an escape. Returns the content end and the token end.
fn scan_quoted(s: &str, open: usize, closer: char) -> (usize, usize) {
    let mut i = open + 1;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        if c == closer {
            if s[i + 1..].starts_with(closer) {
                i += 2;
                continue;
            }
            return (i, i + 1);
        }
        i += c.len_utf8();
    }
    (s.len(), s.len())
}

fn lex(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().unwrap();
        if c.is_whitespace() {
            let n = rest
                .find(|c: char| !c.is_whitespace())
                .unwrap_or(rest.len());
            out.push(tok(Kind::Space, i, i + n));
            i += n;
        } else if rest.starts_with("--") {
            let n = rest.find('\n').unwrap_or(rest.len());
            out.push(tok(Kind::Comment, i, i + n));
            i += n;
        } else if rest.starts_with("/*") {
            // Block comments nest in T-SQL.
            let (mut depth, mut j) = (1, 2);
            while j < rest.len() && depth > 0 {
                if rest[j..].starts_with("/*") {
                    depth += 1;
                    j += 2;
                } else if rest[j..].starts_with("*/") {
                    depth -= 1;
                    j += 2;
                } else {
                    j += rest[j..].chars().next().unwrap().len_utf8();
                }
            }
            out.push(tok(Kind::Comment, i, i + j));
            i += j;
        } else if c == '\'' || ((c == 'N' || c == 'n') && rest[1..].starts_with('\'')) {
            let quote = if c == '\'' { i } else { i + 1 };
            let (end, hi) = scan_quoted(s, quote, '\'');
            out.push(Tok {
                kind: Kind::Str,
                lo: i,
                hi,
                start: quote + 1,
                end,
            });
            i = hi;
        } else if c == '[' || c == '"' {
            let closer = if c == '[' { ']' } else { '"' };
            let (end, hi) = scan_quoted(s, i, closer);
            out.push(Tok {
                kind: Kind::Ident,
                lo: i,
                hi,
                start: i + 1,
                end,
            });
            i = hi;
        } else if c == '@' {
            let n = rest[1..]
                .find(|c: char| !is_word_char(c) && c != '@')
                .map_or(rest.len(), |n| n + 1);
            out.push(tok(Kind::Var, i, i + n));
            i += n;
        } else if is_word_char(c) {
            let n = rest.find(|c: char| !is_word_char(c)).unwrap_or(rest.len());
            out.push(tok(Kind::Word, i, i + n));
            i += n;
        } else {
            out.push(tok(Kind::Punct, i, i + c.len_utf8()));
            i += c.len_utf8();
        }
    }
    out
}

/// Returns `sql` with secret literals replaced by `***`. Quotes are kept so
/// the statement still reads naturally.
pub fn mask_secrets(sql: &str) -> String {
    mask_depth(sql, 0)
}

fn mask_depth(sql: &str, depth: usize) -> String {
    let toks = lex(sql);
    // Tokens that matter for the rules: no spaces, no comments.
    let sig: Vec<usize> = (0..toks.len())
        .filter(|&i| !matches!(toks[i].kind, Kind::Space | Kind::Comment))
        .collect();
    let mut masked = vec![false; toks.len()];

    let name = |t: &Tok| -> Option<String> {
        matches!(t.kind, Kind::Word | Kind::Ident).then(|| sql[t.start..t.end].to_ascii_lowercase())
    };
    let is_punct = |i: usize, ch: char| {
        sig.get(i)
            .is_some_and(|&t| toks[t].kind == Kind::Punct && sql[toks[t].lo..].starts_with(ch))
    };
    let is_str = |i: usize| sig.get(i).is_some_and(|&t| toks[t].kind == Kind::Str);

    for p in 0..sig.len() {
        let t = &toks[sig[p]];
        let Some(n) = name(t) else { continue };

        // WORD = '...'
        if t.kind == Kind::Word && SECRET_KEYWORDS.contains(&n.as_str()) {
            if is_punct(p + 1, '=') && is_str(p + 2) {
                masked[sig[p + 2]] = true;
            }
            continue;
        }

        // OPENROWSET('provider', 'server'; 'user'; 'password', ...)
        if n == "openrowset"
            && is_punct(p + 1, '(')
            && is_str(p + 2)
            && is_punct(p + 3, ',')
            && is_str(p + 4)
            && is_punct(p + 5, ';')
            && is_str(p + 6)
            && is_punct(p + 7, ';')
            && is_str(p + 8)
        {
            masked[sig[p + 8]] = true;
            continue;
        }

        // sp_addlogin 'name', 'secret', ...
        let Some(&(_, positions, names)) = SECRET_PROCS.iter().find(|(proc, _, _)| *proc == n)
        else {
            continue;
        };
        let (mut q, mut position) = (p + 1, 0);
        while let Some(&first) = sig.get(q) {
            let secret = if toks[first].kind == Kind::Var && is_punct(q + 1, '=') {
                let var = sql[toks[first].lo + 1..toks[first].hi].to_ascii_lowercase();
                q += 2;
                names.contains(&var.as_str())
            } else {
                position += 1;
                positions.contains(&(position - 1))
            };
            let Some(&v) = sig.get(q) else { break };
            match toks[v].kind {
                Kind::Str => masked[v] = secret,
                // A bare word is only taken as an argument after a comma, so
                // `EXEC sp_password` followed by another statement is left
                // alone.
                Kind::Word => masked[v] = secret && q != p + 1,
                Kind::Var => {}
                _ => break,
            }
            q += 1;
            if is_punct(q, ',') {
                q += 1;
            } else {
                break;
            }
        }
    }

    let mut out = String::with_capacity(sql.len());
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            Kind::Str => {
                out.push_str(&sql[t.lo..t.start]);
                if masked[i] {
                    out.push_str(MASK);
                } else {
                    out.push_str(&mask_literal(&sql[t.start..t.end], depth));
                }
                out.push_str(&sql[t.end..t.hi]);
            }
            Kind::Word if masked[i] => out.push_str(MASK),
            _ => out.push_str(&sql[t.lo..t.hi]),
        }
    }
    out
}

/// Masks what a string literal that is not a secret itself may carry: a
/// connection string, or SQL run dynamically.
fn mask_literal(content: &str, depth: usize) -> String {
    if depth < MAX_DEPTH {
        let inner = content.replace("''", "'");
        let masked = mask_depth(&inner, depth + 1);
        // Dynamic SQL: its own literals were handled by the inner pass, and
        // reading it as a connection string too would eat its quotes.
        if masked != inner {
            return masked.replace('\'', "''");
        }
    }
    mask_conn_string(content)
}

/// Masks the value of `Password=` / `PWD=` keys in a connection string given
/// as the (still quote-escaped) content of a literal.
fn mask_conn_string(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let key = CONN_KEYS.iter().find(|k| {
            lower[i..].starts_with(**k) && !s[..i].chars().next_back().is_some_and(is_word_char)
        });
        if let Some(key) = key {
            let after_key = i + key.len();
            let tail = &s[after_key..];
            let ws = tail.len() - tail.trim_start().len();
            if tail[ws..].starts_with('=') {
                let value_at = after_key + ws + 1;
                let value = &s[value_at..];
                let lead = value.len() - value.trim_start().len();
                let v = value_at + lead;
                let end = value_end(&s[v..]) + v;
                out.push_str(&s[i..v]);
                out.push_str(MASK);
                i = end;
                continue;
            }
        }
        let c = s[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Length of a connection string value at the start of `v`: braced, quoted
/// (a doubled single quote is how a `'` value sits inside a SQL literal), or
/// bare up to the next `;`.
fn value_end(v: &str) -> usize {
    let delimited = |open: &str, close: &str| {
        v.strip_prefix(open).map(|rest| {
            rest.find(close)
                .map_or(v.len(), |n| open.len() + n + close.len())
        })
    };
    delimited("{", "}")
        .or_else(|| delimited("''", "''"))
        .or_else(|| delimited("\"", "\""))
        .unwrap_or_else(|| v.find(';').unwrap_or(v.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> String {
        mask_secrets(s)
    }

    #[test]
    fn create_login_password() {
        assert_eq!(
            m("CREATE LOGIN x WITH PASSWORD = 'Secret1', CHECK_POLICY = OFF"),
            "CREATE LOGIN x WITH PASSWORD = '***', CHECK_POLICY = OFF"
        );
    }

    #[test]
    fn alter_login_old_and_new_with_n_prefix() {
        assert_eq!(
            m("alter login x with password=N'new' old_password = 'old'"),
            "alter login x with password=N'***' old_password = '***'"
        );
    }

    #[test]
    fn encryption_and_decryption_by_password() {
        assert_eq!(
            m("CREATE MASTER KEY ENCRYPTION BY PASSWORD = 'a b'"),
            "CREATE MASTER KEY ENCRYPTION BY PASSWORD = '***'"
        );
        assert_eq!(
            m("OPEN SYMMETRIC KEY k DECRYPTION BY PASSWORD='p'"),
            "OPEN SYMMETRIC KEY k DECRYPTION BY PASSWORD='***'"
        );
    }

    #[test]
    fn credential_secret_and_backup_passwords() {
        assert_eq!(
            m("CREATE CREDENTIAL c WITH IDENTITY = 'u', SECRET = 's3'"),
            "CREATE CREDENTIAL c WITH IDENTITY = 'u', SECRET = '***'"
        );
        assert_eq!(
            m("BACKUP DATABASE d TO DISK='f' WITH MEDIAPASSWORD='m', PASSWORD='p'"),
            "BACKUP DATABASE d TO DISK='f' WITH MEDIAPASSWORD='***', PASSWORD='***'"
        );
    }

    #[test]
    fn escaped_quotes_inside_the_secret() {
        assert_eq!(
            m("CREATE LOGIN x WITH PASSWORD = 'it''s; a ''secret''' , DEFAULT_DATABASE = d"),
            "CREATE LOGIN x WITH PASSWORD = '***' , DEFAULT_DATABASE = d"
        );
    }

    #[test]
    fn comments_between_keyword_and_literal() {
        assert_eq!(
            m("WITH PASSWORD /* hi */ = -- x\n 'p'"),
            "WITH PASSWORD /* hi */ = -- x\n '***'"
        );
    }

    #[test]
    fn commented_out_or_quoted_keyword_is_not_a_secret() {
        let sql = "-- PASSWORD = 'x'\nSELECT 'PASSWORD' /* PASSWORD = 'y' */, [password]";
        assert_eq!(m(sql), sql);
    }

    #[test]
    fn variables_and_plain_sql_are_untouched() {
        for sql in [
            "SELECT 1",
            "CREATE LOGIN x WITH PASSWORD = @pwd",
            "SELECT 'it''s' AS [Password Reset], N'x'",
            "SELECT ünï, 'çafé'",
        ] {
            assert_eq!(m(sql), sql);
        }
    }

    #[test]
    fn sp_addlogin_positional_and_named() {
        assert_eq!(
            m("EXEC sp_addlogin 'joe', 'pw', 'master'"),
            "EXEC sp_addlogin 'joe', '***', 'master'"
        );
        assert_eq!(
            m("exec master.dbo.sp_addlogin @loginame = 'joe', @passwd = N'pw'"),
            "exec master.dbo.sp_addlogin @loginame = 'joe', @passwd = N'***'"
        );
        assert_eq!(
            m("EXEC [sp_addlogin] joe, secret"),
            "EXEC [sp_addlogin] joe, ***"
        );
    }

    #[test]
    fn sp_password_masks_both() {
        assert_eq!(
            m("EXEC sp_password 'old', 'new', 'joe'"),
            "EXEC sp_password '***', '***', 'joe'"
        );
        assert_eq!(
            m("EXEC sp_password NULL, @new = 'n', @loginame = 'joe'"),
            "EXEC sp_password NULL, @new = '***', @loginame = 'joe'"
        );
    }

    #[test]
    fn sp_addlinkedsrvlogin_and_app_roles() {
        assert_eq!(
            m("EXEC sp_addlinkedsrvlogin 'srv', 'false', NULL, 'u', 'pw'"),
            "EXEC sp_addlinkedsrvlogin 'srv', 'false', NULL, 'u', '***'"
        );
        assert_eq!(
            m("EXEC sp_setapprole 'role', 'pw'"),
            "EXEC sp_setapprole 'role', '***'"
        );
    }

    #[test]
    fn procedure_without_arguments_leaves_the_next_statement() {
        let sql = "EXEC sp_password\nSELECT 1";
        assert_eq!(m(sql), sql);
    }

    #[test]
    fn connection_strings_in_openrowset_and_opendatasource() {
        assert_eq!(
            m("SELECT * FROM OPENROWSET('SQLNCLI', 'Server=s;Uid=u;Pwd=p;', 'select 1')"),
            "SELECT * FROM OPENROWSET('SQLNCLI', 'Server=s;Uid=u;Pwd=***;', 'select 1')"
        );
        assert_eq!(
            m("OPENDATASOURCE('SQLNCLI','Data Source=s;User ID=u; Password = p1 ;Encrypt=yes').db.dbo.t"),
            "OPENDATASOURCE('SQLNCLI','Data Source=s;User ID=u; Password = ***;Encrypt=yes').db.dbo.t"
        );
    }

    #[test]
    fn connection_string_value_forms() {
        assert_eq!(m("'password={a;b};x=1'"), "'password=***;x=1'");
        assert_eq!(m("'Password=\"a;b\";x=1'"), "'Password=***;x=1'");
        assert_eq!(m("'PWD=''a;b'';x=1'"), "'PWD=***;x=1'");
        assert_eq!(m("'Password=abc'"), "'Password=***'");
    }

    #[test]
    fn key_must_stand_alone() {
        let sql = "'MyPassword=abc;Mypwd=1'";
        assert_eq!(m(sql), sql);
    }

    #[test]
    fn old_style_openrowset_triple() {
        assert_eq!(
            m("SELECT * FROM OPENROWSET('SQLOLEDB', 'srv'; 'user'; 'pw', 'select 1')"),
            "SELECT * FROM OPENROWSET('SQLOLEDB', 'srv'; 'user'; '***', 'select 1')"
        );
    }

    #[test]
    fn dynamic_sql_in_a_literal() {
        assert_eq!(
            m("EXEC('CREATE LOGIN x WITH PASSWORD = ''abc''')"),
            "EXEC('CREATE LOGIN x WITH PASSWORD = ''***''')"
        );
        assert_eq!(
            m("EXEC sp_executesql N'ALTER LOGIN x WITH PASSWORD = ''a'' OLD_PASSWORD = ''b'''"),
            "EXEC sp_executesql N'ALTER LOGIN x WITH PASSWORD = ''***'' OLD_PASSWORD = ''***'''"
        );
    }

    #[test]
    fn unterminated_literal_is_masked_to_the_end() {
        assert_eq!(m("WITH PASSWORD = 'abc"), "WITH PASSWORD = '***");
    }

    #[test]
    fn many_statements_and_unicode() {
        assert_eq!(
            m("SELECT 'é';\nCREATE LOGIN ñ WITH PASSWORD = 'pä'; SELECT 2"),
            "SELECT 'é';\nCREATE LOGIN ñ WITH PASSWORD = '***'; SELECT 2"
        );
    }

    #[test]
    fn masking_twice_changes_nothing() {
        let once = m("CREATE LOGIN x WITH PASSWORD = 'a''b'; EXEC sp_addlogin 'u','p'");
        assert_eq!(m(&once), once);
    }
}
