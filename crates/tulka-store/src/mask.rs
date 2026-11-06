//! Hides secret literals in SQL text before it is stored in the run log.
//! The statement that is executed is never touched: only the stored copy is.

use std::collections::HashSet;

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
    ("sp_approlepassword", &[1], &["newpwd"]),
    ("sp_change_users_login", &[3], &["password"]),
];

/// Functions that take a password literal: the 0-based position of the
/// argument.
const SECRET_FUNCTIONS: &[(&str, usize)] = &[
    ("encryptbypassphrase", 0),
    ("decryptbypassphrase", 0),
    ("decryptbykeyautocert", 1),
    ("decryptbykeyautoasymkey", 1),
    ("decryptbycert", 2),
    ("decryptbyasymkey", 2),
    ("signbycert", 2),
    ("signbyasymkey", 2),
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
    let mut vars = HashSet::new();
    collect_secret_vars(sql, 0, &mut vars);
    mask_depth(sql, 0, &vars)
}

/// Names (lowercase, without the `@`) of variables used as `PASSWORD = @x`,
/// anywhere in `sql` or in the dynamic SQL inside its literals. The value
/// bound to such a variable is a secret wherever it is assigned.
fn collect_secret_vars(sql: &str, depth: usize, vars: &mut HashSet<String>) {
    let toks = lex(sql);
    let sig: Vec<&Tok> = toks
        .iter()
        .filter(|t| !matches!(t.kind, Kind::Space | Kind::Comment))
        .collect();
    for (p, t) in sig.iter().enumerate() {
        match t.kind {
            Kind::Word
                if SECRET_KEYWORDS.contains(&sql[t.lo..t.hi].to_ascii_lowercase().as_str()) =>
            {
                if let (Some(eq), Some(var)) = (sig.get(p + 1), sig.get(p + 2)) {
                    if eq.kind == Kind::Punct
                        && sql[eq.lo..].starts_with('=')
                        && var.kind == Kind::Var
                    {
                        vars.insert(sql[var.lo + 1..var.hi].to_ascii_lowercase());
                    }
                }
            }
            Kind::Str if depth < MAX_DEPTH => {
                collect_secret_vars(&sql[t.start..t.end].replace("''", "'"), depth + 1, vars);
            }
            _ => {}
        }
    }
}

/// Whether a variable of this name holds a secret.
fn secret_var(name: &str, vars: &HashSet<String>) -> bool {
    let name = name.to_ascii_lowercase();
    ["pass", "pwd", "secret"].iter().any(|k| name.contains(k)) || vars.contains(&name)
}

fn mask_depth(sql: &str, depth: usize, vars: &HashSet<String>) -> String {
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
    // The ODBC form `{Encrypt N'pw'}` at sig position `i`: the literal's token
    // and how many tokens the form spans.
    let encrypt_form = |i: usize| -> Option<(usize, usize)> {
        let word = sig.get(i + 1).map(|&t| &toks[t])?;
        (is_punct(i, '{')
            && word.kind == Kind::Word
            && sql[word.lo..word.hi].eq_ignore_ascii_case("encrypt")
            && is_str(i + 2)
            && is_punct(i + 3, '}'))
        .then(|| (sig[i + 2], 4))
    };
    // The token of a secret value written at sig position `i`: a literal, a
    // double-quoted literal, a hashed `0x...` value or an encrypted literal.
    // Never `NULL` or a variable.
    // A chain `'a' + 'b'` gives all its literals.
    let secret_values = |i: usize| -> Vec<usize> {
        let Some(&t) = sig.get(i) else {
            return Vec::new();
        };
        let tk = &toks[t];
        match tk.kind {
            Kind::Str => {
                let mut lits = vec![t];
                let mut q = i + 1;
                while is_punct(q, '+') && is_str(q + 1) {
                    lits.push(sig[q + 1]);
                    q += 2;
                }
                lits
            }
            Kind::Ident if sql[tk.lo..].starts_with('"') => vec![t],
            Kind::Word if is_hex(&sql[tk.lo..tk.hi]) => vec![t],
            Kind::Punct => encrypt_form(i).map(|(lit, _)| lit).into_iter().collect(),
            _ => Vec::new(),
        }
    };

    for p in 0..sig.len() {
        let t = &toks[sig[p]];

        // @password = '...', also `DECLARE @pw sysname = '...'`
        if t.kind == Kind::Var && secret_var(&sql[t.lo + 1..t.hi], vars) {
            let mut q = p + 1;
            if sig.get(q).is_some_and(|&i| {
                toks[i].kind == Kind::Word && sql[toks[i].lo..toks[i].hi].eq_ignore_ascii_case("as")
            }) {
                q += 1;
            }
            // An optional type, with its length: `nvarchar(50)`.
            if sig.get(q).is_some_and(|&i| toks[i].kind == Kind::Word) {
                q += 1;
                if is_punct(q, '(') {
                    while q < sig.len() && !is_punct(q, ')') {
                        q += 1;
                    }
                    q += 1;
                }
            }
            if is_punct(q, '=') {
                for lit in secret_values(q + 1) {
                    masked[lit] = true;
                }
            }
            continue;
        }

        let Some(n) = name(t) else { continue };

        // WORD = '...'
        if t.kind == Kind::Word && SECRET_KEYWORDS.contains(&n.as_str()) {
            if is_punct(p + 1, '=') {
                for lit in secret_values(p + 2) {
                    masked[lit] = true;
                }
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

        // ENCRYPTBYPASSPHRASE(N'phrase', ...)
        let function = SECRET_FUNCTIONS
            .iter()
            .find(|(f, _)| *f == n)
            .filter(|_| is_punct(p + 1, '('));
        if let Some(&(_, at)) = function {
            let (mut level, mut arg, mut start) = (0, 0, false);
            for q in p + 1..sig.len() {
                if is_punct(q, '(') {
                    level += 1;
                    start = level == 1;
                    continue;
                }
                if is_punct(q, ')') {
                    level -= 1;
                    if level == 0 {
                        break;
                    }
                } else if is_punct(q, ',') && level == 1 {
                    arg += 1;
                    start = true;
                    continue;
                } else if start && arg == at && toks[sig[q]].kind == Kind::Str {
                    let lits = secret_values(q);
                    let after = q + 2 * lits.len() - 1;
                    if is_punct(after, ',') || is_punct(after, ')') {
                        for lit in lits {
                            masked[lit] = true;
                        }
                    }
                    break;
                }
                start = false;
            }
            continue;
        }

        // sp_executesql @stmt, N'@p sysname', N'value': positional values
        // belong to the parameters declared in the second argument.
        if n == "sp_executesql" {
            let (mut q, mut k) = (p + 1, 0);
            let mut declared: Vec<String> = Vec::new();
            while let Some(&a) = sig.get(q) {
                if toks[a].kind == Kind::Var && is_punct(q + 1, '=') {
                    break;
                }
                if toks[a].kind == Kind::Str {
                    if k == 1 {
                        declared = declared_params(&sql[toks[a].start..toks[a].end]);
                    } else if k >= 2
                        && declared
                            .get(k - 2)
                            .is_some_and(|name| secret_var(name, vars))
                    {
                        masked[a] = true;
                    }
                }
                q += 1;
                if is_punct(q, ',') {
                    q += 1;
                    k += 1;
                } else {
                    break;
                }
            }
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
                names.contains(&var.as_str()) || secret_var(&var, vars)
            } else {
                position += 1;
                positions.contains(&(position - 1))
            };
            let Some(&v) = sig.get(q) else { break };
            match toks[v].kind {
                Kind::Str => masked[v] = secret,
                Kind::Ident if sql[toks[v].lo..].starts_with('"') => masked[v] = secret,
                // A bare word is only taken as an argument after a comma, so
                // `EXEC sp_password` followed by another statement is left
                // alone. `NULL` and `DEFAULT` are not secrets.
                Kind::Word => {
                    let w = &sql[toks[v].lo..toks[v].hi];
                    masked[v] = secret
                        && q != p + 1
                        && !w.eq_ignore_ascii_case("null")
                        && !w.eq_ignore_ascii_case("default");
                }
                Kind::Var => {}
                Kind::Punct => match encrypt_form(q) {
                    Some((lit, spanned)) => {
                        masked[lit] = secret;
                        q += spanned - 1;
                    }
                    None => break,
                },
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
        let text = &sql[t.lo..t.hi];
        match t.kind {
            Kind::Str => {
                out.push_str(&sql[t.lo..t.start]);
                if masked[i] {
                    out.push_str(MASK);
                } else {
                    out.push_str(&mask_literal(&sql[t.start..t.end], depth, vars));
                }
                out.push_str(&sql[t.end..t.hi]);
            }
            Kind::Ident if masked[i] => {
                out.push_str(&sql[t.lo..t.start]);
                out.push_str(MASK);
                out.push_str(&sql[t.end..t.hi]);
            }
            Kind::Word if masked[i] => out.push_str(if is_hex(text) { "0x***" } else { MASK }),
            Kind::Comment => out.push_str(&mask_comment(text, depth, vars)),
            _ => out.push_str(text),
        }
    }
    out
}

/// `0x` followed by at least one more char: a hashed password. Compared as
/// bytes, since the word may start with a multi-byte char.
fn is_hex(word: &str) -> bool {
    word.len() > 2
        && word
            .as_bytes()
            .get(..2)
            .is_some_and(|b| b.eq_ignore_ascii_case(b"0x"))
}

/// A comment may hold commented-out SQL with a secret in it. Its body is
/// masked like any other text, one level deeper like dynamic SQL, so nested
/// comments cannot recurse without bound.
fn mask_comment(text: &str, depth: usize, vars: &HashSet<String>) -> String {
    if depth >= MAX_DEPTH {
        return text.to_string();
    }
    if let Some(body) = text.strip_prefix("--") {
        return format!("--{}", mask_depth(body, depth + 1, vars));
    }
    // A block comment may be unterminated, running to the end of the text.
    let body = &text[2..];
    match body.strip_suffix("*/") {
        Some(body) => format!("/*{}*/", mask_depth(body, depth + 1, vars)),
        None => format!("/*{}", mask_depth(body, depth + 1, vars)),
    }
}

/// The parameter names (lowercase, no `@`) of a `sp_executesql` declaration
/// list such as `@a int, @b nvarchar(10) OUTPUT`.
fn declared_params(decl: &str) -> Vec<String> {
    let (mut names, mut level, mut expect) = (Vec::new(), 0, true);
    for t in lex(decl) {
        let text = &decl[t.lo..t.hi];
        match t.kind {
            Kind::Space | Kind::Comment => {}
            Kind::Var if expect && level == 0 => {
                names.push(text[1..].to_ascii_lowercase());
                expect = false;
            }
            Kind::Punct if text == "(" => level += 1,
            Kind::Punct if text == ")" && level > 0 => level -= 1,
            Kind::Punct if text == "," && level == 0 => expect = true,
            _ => expect = false,
        }
    }
    names
}

/// Masks what a string literal that is not a secret itself may carry: SQL run
/// dynamically, or a connection string (or both).
fn mask_literal(content: &str, depth: usize, vars: &HashSet<String>) -> String {
    if depth < MAX_DEPTH {
        let inner = content.replace("''", "'");
        let masked = mask_depth(&inner, depth + 1, vars);
        if masked != inner {
            return mask_conn_string(&masked.replace('\'', "''"), true);
        }
    }
    mask_conn_string(content, false)
}

/// Masks the value of `Password=` / `PWD=` keys in a connection string given
/// as the (still quote-escaped) content of a literal. `is_sql` is set when
/// the literal is dynamic SQL, where `password = @p` is a variable.
fn mask_conn_string(s: &str, is_sql: bool) -> String {
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
                if !(is_sql && s[v..].starts_with('@'))
                    && !is_sql_variable(&s[v..], ws > 0 && lead > 0)
                    && !is_masked(&s[v..])
                {
                    let end = value_end(&s[v..]) + v;
                    out.push_str(&s[i..v]);
                    out.push_str(MASK);
                    i = end;
                    continue;
                }
            }
        }
        let c = s[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `@name` followed by a space, `)` or `,` (or ending the text, when written
/// `password = @p`): SQL such as `password = @p AND`,
/// not a connection string value. `Password=@Secr3t;` is a value.
fn is_sql_variable(v: &str, spaced: bool) -> bool {
    let Some(rest) = v.strip_prefix('@') else {
        return false;
    };
    let rest = rest.trim_start_matches(is_word_char);
    // At the very end only the spaced form `password = @p` reads as SQL.
    (rest.is_empty() && spaced)
        || rest.starts_with(|c: char| c.is_whitespace() || c == ')' || c == ',')
}

/// Exactly `***`, plain or inside the delimiters a value may have.
fn is_masked(v: &str) -> bool {
    let probe = v.trim_start_matches(['\'', '"', '{']);
    probe
        .strip_prefix(MASK)
        .is_some_and(|rest| !rest.starts_with(is_word_char))
}

/// Length of a connection string value at the start of `v`: braced or
/// double-quoted (a doubled closer is an escape), quoted with a doubled single
/// quote (how a `'` sits inside a SQL literal; four of them are an escaped
/// quote), or bare up to the next `;`.
fn value_end(v: &str) -> usize {
    let delimited = |open: &str, close: &str, escaped: &str| {
        v.strip_prefix(open).map(|rest| {
            let mut i = 0;
            while i < rest.len() {
                if rest[i..].starts_with(escaped) {
                    i += escaped.len();
                } else if rest[i..].starts_with(close) {
                    return open.len() + i + close.len();
                } else {
                    i += rest[i..].chars().next().unwrap().len_utf8();
                }
            }
            v.len()
        })
    };
    delimited("{", "}", "}}")
        .or_else(|| delimited("''", "''", "''''"))
        .or_else(|| delimited("\"", "\"", "\"\""))
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
        let sql = "SELECT 'PASSWORD', [password], \"password\"";
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
        assert_eq!(m("'Password=\"a;b\";x=1'"), "'Password=\"***\";x=1'");
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

    #[test]
    fn variables_named_like_secrets_are_masked() {
        assert_eq!(m("SET @password = 'x'"), "SET @password = '***'");
        assert_eq!(
            m("EXEC dbo.CreateAppUser @UserName='u', @Password='S3cret!'"),
            "EXEC dbo.CreateAppUser @UserName='u', @Password='***'"
        );
        assert_eq!(
            m("EXEC sp_addpushsubscription_agent @subscriber_password=N'p', @job_password=N'j'"),
            "EXEC sp_addpushsubscription_agent @subscriber_password=N'***', @job_password=N'***'"
        );
        assert_eq!(
            m("EXEC sp_adddistributor @distributor = 'd', @password = 'p'"),
            "EXEC sp_adddistributor @distributor = 'd', @password = '***'"
        );
        assert_eq!(
            m("EXEC sp_control_dbmasterkey_password @db_name = 'd', @password = 'p', @action = 'add'"),
            "EXEC sp_control_dbmasterkey_password @db_name = 'd', @password = '***', @action = 'add'"
        );
    }

    #[test]
    fn declare_masks_the_assigned_literal() {
        assert_eq!(
            m("DECLARE @pwd nvarchar(50) = N'S3cret!', @n int = 1"),
            "DECLARE @pwd nvarchar(50) = N'***', @n int = 1"
        );
        assert_eq!(
            m("DECLARE @n int = 1, @secret AS sysname = 'x'"),
            "DECLARE @n int = 1, @secret AS sysname = '***'"
        );
    }

    #[test]
    fn a_variable_used_after_password_is_secret_where_it_is_assigned() {
        assert_eq!(
            m("DECLARE @pw sysname = N'S3cret!'; CREATE LOGIN x WITH PASSWORD = @pw"),
            "DECLARE @pw sysname = N'***'; CREATE LOGIN x WITH PASSWORD = @pw"
        );
        assert_eq!(
            m("EXEC sp_executesql N'CREATE LOGIN x WITH PASSWORD = @p', N'@p sysname', @p = N'S3cret!'"),
            "EXEC sp_executesql N'CREATE LOGIN x WITH PASSWORD = @p', N'@p sysname', @p = N'***'"
        );
    }

    #[test]
    fn other_variables_and_nulls_are_left_alone() {
        for sql in [
            "SET @name = 'x'",
            "SET @password = @other",
            "SET @password = NULL",
            "EXEC sp_addlogin 'joe', NULL",
            "SELECT @password AS p, name = 'x'",
            "EXEC('SELECT 1 WHERE password = @p AND id = 1;')",
            "SELECT 'password = @p WHERE id = 1;'",
        ] {
            assert_eq!(m(sql), sql, "{sql}");
        }
    }

    #[test]
    fn more_known_procedures() {
        assert_eq!(
            m("EXEC sp_change_users_login 'Auto_Fix', 'u', NULL, 'pw'"),
            "EXEC sp_change_users_login 'Auto_Fix', 'u', NULL, '***'"
        );
        assert_eq!(
            m("EXEC sp_approlepassword 'role', 'pw'"),
            "EXEC sp_approlepassword 'role', '***'"
        );
    }

    #[test]
    fn encryption_functions() {
        assert_eq!(
            m("SELECT ENCRYPTBYPASSPHRASE(N'phrase', 'data'), DECRYPTBYPASSPHRASE('p2', c)"),
            "SELECT ENCRYPTBYPASSPHRASE(N'***', 'data'), DECRYPTBYPASSPHRASE('***', c)"
        );
        assert_eq!(
            m("SELECT DECRYPTBYKEYAUTOCERT(CERT_ID('c'), N'pw', col)"),
            "SELECT DECRYPTBYKEYAUTOCERT(CERT_ID('c'), N'***', col)"
        );
        assert_eq!(
            m("SELECT DECRYPTBYKEYAUTOASYMKEY(AsymKey_ID('k'), N'pw', col)"),
            "SELECT DECRYPTBYKEYAUTOASYMKEY(AsymKey_ID('k'), N'***', col)"
        );
        assert_eq!(
            m("SELECT DECRYPTBYCERT(1, c, N'pw'), DECRYPTBYASYMKEY(1, c, 'pw'), SIGNBYCERT(1, x, 'pw'), SIGNBYASYMKEY(1, x, 'pw')"),
            "SELECT DECRYPTBYCERT(1, c, N'***'), DECRYPTBYASYMKEY(1, c, '***'), SIGNBYCERT(1, x, '***'), SIGNBYASYMKEY(1, x, '***')"
        );
        let untouched = "SELECT ENCRYPTBYPASSPHRASE(@p, 'data'), DECRYPTBYCERT(1, c)";
        assert_eq!(m(untouched), untouched);
    }

    #[test]
    fn app_role_odbc_encrypt_form() {
        assert_eq!(
            m("EXEC sp_setapprole 'role', {Encrypt N'pw'}, 'odbc'"),
            "EXEC sp_setapprole 'role', {Encrypt N'***'}, 'odbc'"
        );
        assert_eq!(
            m("EXEC sp_setapprole @rolename = 'role', @password = {Encrypt N'pw'}, @encrypt = 'odbc'"),
            "EXEC sp_setapprole @rolename = 'role', @password = {Encrypt N'***'}, @encrypt = 'odbc'"
        );
    }

    #[test]
    fn escaped_connection_string_values() {
        assert_eq!(m("'Pwd={a}}b;c};x=1'"), "'Pwd=***;x=1'");
        assert_eq!(m("'Password=\"a\"\"b\";x=1'"), "'Password=\"***\";x=1'");
        assert_eq!(m("'Pwd=''a''''b;c'';x=1'"), "'Pwd=***;x=1'");
    }

    #[test]
    fn literal_that_is_dynamic_sql_and_a_connection_string() {
        assert_eq!(
            m("EXEC('CREATE LOGIN x WITH PASSWORD = ''a''; SELECT * FROM OPENROWSET(''p'', ''Pwd=zz;'', ''q'')')"),
            "EXEC('CREATE LOGIN x WITH PASSWORD = ''***''; SELECT * FROM OPENROWSET(''p'', ''Pwd=***;'', ''q'')')"
        );
        assert_eq!(
            m("EXEC('Pwd=zz; PASSWORD = ''a''')"),
            "EXEC('Pwd=***; PASSWORD = ''***''')"
        );
    }

    #[test]
    fn hashed_passwords() {
        assert_eq!(
            m("CREATE LOGIN x WITH PASSWORD = 0x0200ABCD HASHED, SID = 0x01"),
            "CREATE LOGIN x WITH PASSWORD = 0x*** HASHED, SID = 0x01"
        );
    }

    #[test]
    fn double_quoted_secret_value() {
        assert_eq!(
            m("CREATE LOGIN x WITH PASSWORD = \"p\"\"q\""),
            "CREATE LOGIN x WITH PASSWORD = \"***\""
        );
    }

    #[test]
    fn unterminated_block_comment_is_still_masked() {
        assert_eq!(
            m("SELECT 1 /* CREATE LOGIN x WITH PASSWORD = 'p'"),
            "SELECT 1 /* CREATE LOGIN x WITH PASSWORD = '***'"
        );
        assert_eq!(
            m("SELECT 1 /* PASSWORD = 'p' */"),
            "SELECT 1 /* PASSWORD = '***' */"
        );
    }

    #[test]
    fn masking_is_idempotent() {
        for sql in [
            "'Password='''''",
            "'Password=\"\"'",
            "CREATE LOGIN x WITH PASSWORD = 0xAB HASHED",
            "EXEC('CREATE LOGIN x WITH PASSWORD = ''a''; SELECT ''Pwd=1;''')",
            "SELECT 1 /* PASSWORD = 'p'",
        ] {
            let once = m(sql);
            assert_eq!(m(&once), once, "{sql}");
        }
    }

    #[test]
    fn closed_comments_are_masked_too() {
        assert_eq!(
            m("-- PASSWORD = 'x'\nSELECT 1 /* old: SECRET = 's' */"),
            "-- PASSWORD = '***'\nSELECT 1 /* old: SECRET = '***' */"
        );
        let plain = "-- just a note\nSELECT 1 /* nothing */";
        assert_eq!(m(plain), plain);
    }

    #[test]
    fn multi_byte_words_do_not_panic() {
        for sql in [
            "SELECT @password = 密码 FROM 用户",
            "UPDATE 用户 SET password = 新密码列",
            "SELECT 'é' + 'ü', @pwd = é, PASSWORD = ñandú",
            "DECLARE @pw nvarchar = 中",
            "SELECT 0é, 0x, 0xé",
        ] {
            assert_eq!(m(sql), sql, "{sql}");
        }
    }

    #[test]
    fn deep_nesting_does_not_overflow_a_small_stack() {
        let inputs = vec![
            "/*".repeat(50_000),
            "'".repeat(50_000),
            "EXEC('".repeat(20_000),
            "/*'".repeat(30_000) + &"EXEC('".repeat(10_000),
            format!(
                "{}PASSWORD = 'x'{}",
                "/*".repeat(10_000),
                "*/".repeat(10_000)
            ),
            "-- ".repeat(20_000) + "/*" + &"'".repeat(1001),
            "(".repeat(50_000) + &"ENCRYPTBYPASSPHRASE(".repeat(5_000),
        ];
        let handle = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || {
                for sql in &inputs {
                    mask_secrets(sql);
                }
            })
            .unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn nested_comments_are_masked_down_to_a_bounded_depth() {
        assert_eq!(
            m("/* a /* b PASSWORD = 'p' */ c */"),
            "/* a /* b PASSWORD = '***' */ c */"
        );
    }

    #[test]
    fn connection_string_with_a_variable_like_value_is_masked() {
        assert_eq!(
            m("SELECT 'Server=s;User ID=u;Password=@Secr3t;'"),
            "SELECT 'Server=s;User ID=u;Password=***;'"
        );
        assert_eq!(m("SELECT 'Password=@Secr3t'"), "SELECT 'Password=***'");
    }

    #[test]
    fn only_an_exact_mask_is_left_alone() {
        assert_eq!(m("'Password=***real;x=1'"), "'Password=***;x=1'");
        assert_eq!(m("'Password={***real};x=1'"), "'Password=***;x=1'");
        let masked = "'Password=***;x=1'";
        assert_eq!(m(masked), masked);
    }

    #[test]
    fn concatenated_literals_in_secret_positions() {
        assert_eq!(
            m("SET @pwd = N'a' + N'b' + N'c'"),
            "SET @pwd = N'***' + N'***' + N'***'"
        );
        assert_eq!(
            m("CREATE LOGIN x WITH PASSWORD = 'a' + 'b', DEFAULT_DATABASE = d"),
            "CREATE LOGIN x WITH PASSWORD = '***' + '***', DEFAULT_DATABASE = d"
        );
        assert_eq!(
            m("SELECT ENCRYPTBYPASSPHRASE(N'a' + N'b', 'text')"),
            "SELECT ENCRYPTBYPASSPHRASE(N'***' + N'***', 'text')"
        );
        let other = "SET @name = 'a' + 'b'";
        assert_eq!(m(other), other);
    }

    #[test]
    fn positional_sp_executesql_values_follow_the_declared_names() {
        assert_eq!(
            m("EXEC sp_executesql N'CREATE LOGIN x WITH PASSWORD = @p', N'@p sysname', N'S3cret!'"),
            "EXEC sp_executesql N'CREATE LOGIN x WITH PASSWORD = @p', N'@p sysname', N'***'"
        );
        assert_eq!(
            m("EXEC sp_executesql N'SELECT 1', N'@id int, @pass nvarchar(20) OUTPUT', 5, N'S3cret!'"),
            "EXEC sp_executesql N'SELECT 1', N'@id int, @pass nvarchar(20) OUTPUT', 5, N'***'"
        );
        let plain = "EXEC sp_executesql N'SELECT @a', N'@a nvarchar(5)', N'hello'";
        assert_eq!(m(plain), plain);
    }
}
