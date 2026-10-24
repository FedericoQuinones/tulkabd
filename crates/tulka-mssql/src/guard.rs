//! Finds the statements of a T-SQL script that are dangerous to run
//! unattended: `UPDATE` or `DELETE` without a `WHERE`, `TRUNCATE` and `DROP`.
//!
//! This is a small lexer plus a statement scanner rather than a full parser:
//! it only has to recognise four statement shapes and where they end, and a
//! real grammar would mis-handle much of everyday T-SQL. When the lexer
//! cannot finish (an unclosed string, identifier or comment) the batch falls
//! back to a keyword search, so a dangerous word is never silently missed.
//!
//! Known limits: `MERGE` is not inspected, and dynamic SQL built from
//! variables (`EXEC(@sql)`) cannot be known ahead of time.

use tulka_core::{GuardFinding, GuardKind};

use crate::batches::split_batches;

/// Longest statement text kept in a finding, in characters.
const MAX_STATEMENT_CHARS: usize = 400;
/// How many levels of `EXEC('...')` are analyzed.
const MAX_DYNAMIC_LEVEL: u8 = 1;

/// Analyzes `sql` (a whole script, `GO` lines included) and returns the
/// flagged statements in text order. Lines are 0-based and relative to `sql`.
pub fn findings(sql: &str) -> Vec<GuardFinding> {
    let mut out = Vec::new();
    for batch in split_batches(sql) {
        analyze_batch(&batch.text, batch.start_line - 1, 0, &mut out);
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Word,
    Quoted,
    Str,
    Var,
    Temp,
    Num,
    Punct,
}

#[derive(Clone, Copy, Debug)]
struct Token {
    kind: Kind,
    start: usize,
    end: usize,
    /// 0-based line within the batch.
    line: u32,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Splits `text` into tokens, skipping comments. `Err` carries the offset of
/// the unclosed string, identifier or comment.
fn lex(text: &str) -> Result<Vec<Token>, usize> {
    let b = text.as_bytes();
    let mut tokens = Vec::with_capacity(text.len() / 4);
    let mut i = 0;
    let mut line = 0u32;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'-' && b.get(i + 1) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let open = i;
            let mut depth = 0u32;
            loop {
                if i >= b.len() {
                    return Err(open);
                }
                if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
        } else if c == b'\'' || c == b'"' || c == b'[' {
            let close = if c == b'[' { b']' } else { c };
            let start = i;
            let start_line = line;
            i += 1;
            loop {
                if i >= b.len() {
                    return Err(start);
                }
                if b[i] == close {
                    if b.get(i + 1) == Some(&close) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                if b[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
            let kind = if c == b'\'' { Kind::Str } else { Kind::Quoted };
            tokens.push(Token {
                kind,
                start,
                end: i,
                line: start_line,
            });
        } else if c == b'@' || c == b'#' {
            let start = i;
            while i < b.len() && (b[i] == c) {
                i += 1;
            }
            i = advance_while(text, i, is_word_char);
            let kind = if c == b'@' { Kind::Var } else { Kind::Temp };
            tokens.push(Token {
                kind,
                start,
                end: i,
                line,
            });
        } else if c.is_ascii_digit() {
            let start = i;
            i = advance_while(text, i, is_word_char);
            tokens.push(Token {
                kind: Kind::Num,
                start,
                end: i,
                line,
            });
        } else {
            let ch = text[i..].chars().next().unwrap();
            if is_word_char(ch) {
                let start = i;
                i = advance_while(text, i, is_word_char);
                // `N'...'` is a Unicode string literal.
                if matches!(&text[start..i], "N" | "n") && b.get(i) == Some(&b'\'') {
                    let mut j = i + 1;
                    let start_line = line;
                    loop {
                        if j >= b.len() {
                            return Err(start);
                        }
                        if b[j] == b'\'' {
                            if b.get(j + 1) == Some(&b'\'') {
                                j += 2;
                                continue;
                            }
                            j += 1;
                            break;
                        }
                        if b[j] == b'\n' {
                            line += 1;
                        }
                        j += 1;
                    }
                    i = j;
                    tokens.push(Token {
                        kind: Kind::Str,
                        start,
                        end: i,
                        line: start_line,
                    });
                    continue;
                }
                tokens.push(Token {
                    kind: Kind::Word,
                    start,
                    end: i,
                    line,
                });
            } else {
                i += ch.len_utf8();
                tokens.push(Token {
                    kind: Kind::Punct,
                    start: i - ch.len_utf8(),
                    end: i,
                    line,
                });
            }
        }
    }
    Ok(tokens)
}

fn advance_while(text: &str, mut i: usize, f: impl Fn(char) -> bool) -> usize {
    for ch in text[i..].chars() {
        if !f(ch) {
            break;
        }
        i += ch.len_utf8();
    }
    i
}

/// Cursor over the tokens of one batch.
struct Scan<'a> {
    text: &'a str,
    toks: Vec<Token>,
}

impl Scan<'_> {
    fn src(&self, i: usize) -> &str {
        &self.text[self.toks[i].start..self.toks[i].end]
    }

    fn is_kw(&self, i: usize, kw: &str) -> bool {
        self.toks.get(i).is_some_and(|t| t.kind == Kind::Word)
            && self.src(i).eq_ignore_ascii_case(kw)
    }

    fn is_any_kw(&self, i: usize, kws: &[&str]) -> bool {
        kws.iter().any(|k| self.is_kw(i, k))
    }

    fn is_punct(&self, i: usize, p: char) -> bool {
        self.toks
            .get(i)
            .is_some_and(|t| t.kind == Kind::Punct && self.text[t.start..t.end].starts_with(p))
    }

    fn is_name_part(&self, i: usize) -> bool {
        self.toks
            .get(i)
            .is_some_and(|t| matches!(t.kind, Kind::Word | Kind::Quoted | Kind::Temp | Kind::Var))
    }

    /// Reads a dotted name (1 to 4 parts) at `i`. Returns the name as written,
    /// whether it is a temporary table or table variable, and the index after it.
    fn name(&self, i: usize) -> Option<(String, bool, usize)> {
        if !self.is_name_part(i) {
            return None;
        }
        let temp = matches!(self.toks[i].kind, Kind::Temp | Kind::Var);
        let mut end = i;
        let mut parts = 1;
        while parts < 4 && self.is_punct(end + 1, '.') {
            // `db..t` leaves the schema out.
            if self.is_punct(end + 2, '.') && self.is_name_part(end + 3) {
                end += 3;
            } else if self.is_name_part(end + 2) {
                end += 2;
            } else {
                break;
            }
            parts += 1;
        }
        let text = self.text[self.toks[i].start..self.toks[end].end].to_string();
        Some((text, temp, end + 1))
    }

    /// Skips an optional `TOP (n)` / `TOP n` at `i`.
    fn skip_top(&self, i: usize) -> usize {
        if !self.is_kw(i, "TOP") {
            return i;
        }
        if self.is_punct(i + 1, '(') {
            let mut depth = 0;
            let mut j = i + 1;
            while j < self.toks.len() {
                if self.is_punct(j, '(') {
                    depth += 1;
                } else if self.is_punct(j, ')') {
                    depth -= 1;
                    if depth == 0 {
                        return j + 1;
                    }
                }
                j += 1;
            }
            return j;
        }
        i + 2
    }

    /// Finds where the statement that starts at `from` ends: the index of the
    /// `;` or of the word that opens the next statement, or the token count.
    /// The statement is an `UPDATE` when `is_update`, whose first `SET` is
    /// part of it. Also reports whether a top-level `WHERE` was seen.
    fn statement_end(&self, from: usize, is_update: bool) -> (usize, bool) {
        const OPENERS: &[&str] = &[
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "MERGE",
            "DROP",
            "TRUNCATE",
            "CREATE",
            "ALTER",
            "EXEC",
            "EXECUTE",
            "DECLARE",
            "IF",
            "ELSE",
            "WHILE",
            "BEGIN",
            "END",
            "RETURN",
            "PRINT",
            "USE",
            "GRANT",
            "COMMIT",
            "ROLLBACK",
            "SAVE",
            "RAISERROR",
            "THROW",
            "WAITFOR",
        ];
        let mut depth = 0i32;
        let mut cases = 0u32;
        let mut seen_set = false;
        let mut where_seen = false;
        let mut j = from + 1;
        while j < self.toks.len() {
            if self.is_punct(j, '(') {
                depth += 1;
            } else if self.is_punct(j, ')') {
                depth -= 1;
            } else if depth <= 0 {
                if self.is_punct(j, ';') {
                    return (j, where_seen);
                }
                if self.toks[j].kind == Kind::Word {
                    if self.is_kw(j, "CASE") {
                        cases += 1;
                    } else if cases > 0 && self.is_kw(j, "END") {
                        cases -= 1;
                    } else if cases > 0 {
                        // ELSE and friends inside a CASE expression.
                    } else if self.is_kw(j, "WHERE") {
                        where_seen = true;
                    } else if self.is_kw(j, "SET") {
                        if is_update && !seen_set {
                            seen_set = true;
                        } else {
                            return (j, where_seen);
                        }
                    } else if self.is_kw(j, "WITH") {
                        if self.toks.get(j + 1).is_some_and(|t| t.kind == Kind::Word) {
                            return (j, where_seen);
                        }
                    } else if self.is_any_kw(j, OPENERS) {
                        return (j, where_seen);
                    }
                }
            }
            j += 1;
        }
        (self.toks.len(), where_seen)
    }
}

fn truncate_statement(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(MAX_STATEMENT_CHARS) {
        Some((cut, _)) => format!("{}…", &s[..cut]),
        None => s.to_string(),
    }
}

fn analyze_batch(text: &str, base_line: u32, level: u8, out: &mut Vec<GuardFinding>) {
    let toks = match lex(text) {
        Ok(t) => t,
        Err(_) => {
            if let Some(pos) = first_dangerous_word(text) {
                let line = text[..pos].bytes().filter(|&c| c == b'\n').count() as u32;
                out.push(GuardFinding {
                    kind: GuardKind::Unparsed,
                    object: None,
                    statement: truncate_statement(&text[pos..]),
                    line: base_line + line,
                });
            }
            return;
        }
    };
    let sc = Scan { text, toks };

    // Creating a module runs nothing; its body is not analyzed.
    let mut k = 0;
    if sc.is_kw(0, "CREATE") {
        k = 1;
        if sc.is_kw(1, "OR") && sc.is_kw(2, "ALTER") {
            k = 3;
        }
    } else if sc.is_kw(0, "ALTER") {
        k = 1;
    }
    if k > 0 && sc.is_any_kw(k, &["PROC", "PROCEDURE", "FUNCTION", "TRIGGER", "VIEW"]) {
        return;
    }

    let mut depth = 0i32;
    // Table named by the `ALTER TABLE` statement being scanned, with the
    // token where it starts.
    let mut alter_table: Option<(usize, Option<String>)> = None;
    let mut i = 0;
    while i < sc.toks.len() {
        if sc.is_punct(i, '(') {
            depth += 1;
            i += 1;
            continue;
        }
        if sc.is_punct(i, ')') {
            depth -= 1;
            i += 1;
            continue;
        }
        if depth == 0 && sc.is_punct(i, ';') {
            alter_table = None;
        }
        if depth > 0 || sc.toks[i].kind != Kind::Word {
            i += 1;
            continue;
        }

        if sc.is_kw(i, "ALTER") && sc.is_kw(i + 1, "TABLE") {
            let table = sc.name(i + 2).map(|(n, _, _)| n);
            alter_table = Some((i, table));
            i += 2;
            continue;
        }

        if sc.is_kw(i, "EXEC") || sc.is_kw(i, "EXECUTE") {
            alter_table = None;
            if level < MAX_DYNAMIC_LEVEL {
                if let Some(inner) = dynamic_sql(&sc, i + 1) {
                    let mut found = Vec::new();
                    analyze_batch(&inner, 0, level + 1, &mut found);
                    let line = base_line + sc.toks[i].line;
                    for mut f in found {
                        f.line = line;
                        out.push(f);
                    }
                }
            }
            i += 1;
            continue;
        }

        let is_dml = sc.is_kw(i, "UPDATE") || sc.is_kw(i, "DELETE");
        let is_start = is_dml || sc.is_kw(i, "TRUNCATE") || sc.is_kw(i, "DROP");
        if !is_start {
            if alter_table.is_some()
                && sc.is_any_kw(
                    i,
                    &[
                        "SELECT", "INSERT", "MERGE", "CREATE", "DECLARE", "IF", "ELSE", "WHILE",
                        "BEGIN", "END", "RETURN", "PRINT", "USE", "GRANT",
                    ],
                )
            {
                alter_table = None;
            }
            i += 1;
            continue;
        }
        alter_table = if sc.is_kw(i, "DROP") {
            alter_table
        } else {
            None
        };

        let prev_blocks = i > 0
            && (sc.is_any_kw(
                i - 1,
                &[
                    "ON", "FOR", "AFTER", "OF", "GRANT", "DENY", "REVOKE", "THEN",
                ],
            ) || sc.is_punct(i - 1, ','));
        if is_dml && prev_blocks {
            i += 1;
            continue;
        }
        if sc.is_kw(i, "UPDATE") && (sc.is_punct(i + 1, '(') || sc.is_kw(i + 1, "STATISTICS")) {
            i += 1;
            continue;
        }

        let (kind, target, start) = if sc.is_kw(i, "UPDATE") {
            (
                GuardKind::UpdateWithoutWhere,
                sc.name(sc.skip_top(i + 1)),
                i,
            )
        } else if sc.is_kw(i, "DELETE") {
            let mut j = sc.skip_top(i + 1);
            if sc.is_kw(j, "FROM") {
                j += 1;
            }
            (GuardKind::DeleteWithoutWhere, sc.name(j), i)
        } else if sc.is_kw(i, "TRUNCATE") {
            let j = if sc.is_kw(i + 1, "TABLE") {
                i + 2
            } else {
                i + 1
            };
            (GuardKind::Truncate, sc.name(j), i)
        } else if let Some((at, table)) = alter_table.clone() {
            // Inside `ALTER TABLE`, only dropping a column loses data.
            if sc.is_any_kw(i + 1, &["CONSTRAINT", "INDEX", "PERIOD"]) {
                i += 1;
                continue;
            }
            let target = table.map(|n| (n, false, 0));
            (GuardKind::Drop, target, at)
        } else {
            let mut j = i + 2;
            if sc.is_kw(j, "IF") && sc.is_kw(j + 1, "EXISTS") {
                j += 2;
            }
            (GuardKind::Drop, sc.name(j), i)
        };

        if target.as_ref().is_some_and(|(_, temp, _)| *temp) {
            i += 1;
            continue;
        }

        let (end, where_seen) = sc.statement_end(i, sc.is_kw(i, "UPDATE"));
        let flagged = match kind {
            GuardKind::UpdateWithoutWhere | GuardKind::DeleteWithoutWhere => !where_seen,
            _ => true,
        };
        if flagged {
            let last = end.saturating_sub(1).max(i);
            let statement = &text[sc.toks[start].start..sc.toks[last].end];
            out.push(GuardFinding {
                kind,
                object: target.map(|(n, _, _)| n),
                statement: truncate_statement(statement),
                line: base_line + sc.toks[start].line,
            });
        }
        i = end.max(i + 1);
    }
}

/// The text of a literal dynamic statement: the argument of `EXEC(...)` or
/// of `sp_executesql`, when it is made only of string literals joined by `+`.
/// `i` is the token after `EXEC`.
fn dynamic_sql(sc: &Scan, i: usize) -> Option<String> {
    let (first, parens) = if sc.is_punct(i, '(') {
        (i + 1, true)
    } else if sc.is_kw(i, "sp_executesql") {
        (i + 1, false)
    } else if sc.is_name_part(i) && sc.is_punct(i + 1, '.') && sc.is_kw(i + 2, "sp_executesql") {
        (i + 3, false)
    } else {
        return None;
    };
    let mut j = first;
    let mut sql = String::new();
    loop {
        let t = sc.toks.get(j)?;
        if t.kind != Kind::Str {
            return None;
        }
        sql.push_str(&unquote(sc.src(j)));
        j += 1;
        if sc.is_punct(j, '+') {
            j += 1;
        } else {
            break;
        }
    }
    // After a literal-only argument comes `)`, `,` or the end of the statement.
    if parens && !sc.is_punct(j, ')') {
        return None;
    }
    Some(sql)
}

/// The value of a string literal token: no `N` prefix, no quotes, `''` as `'`.
fn unquote(lit: &str) -> String {
    let body = lit.trim_start_matches(['N', 'n']);
    body[1..body.len() - 1].replace("''", "'")
}

/// Offset of the first whole word `UPDATE`, `DELETE`, `DROP` or `TRUNCATE`.
fn first_dangerous_word(text: &str) -> Option<usize> {
    let mut start = None;
    for (i, ch) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        match (is_word_char(ch), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                let w = &text[s..i];
                if ["UPDATE", "DELETE", "DROP", "TRUNCATE"]
                    .iter()
                    .any(|k| w.eq_ignore_ascii_case(k))
                {
                    return Some(s);
                }
                start = None;
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(sql: &str) -> Vec<(GuardKind, Option<String>)> {
        findings(sql)
            .into_iter()
            .map(|f| (f.kind, f.object))
            .collect()
    }

    fn want(v: &[(GuardKind, Option<&str>)]) -> Vec<(GuardKind, Option<String>)> {
        v.iter().map(|(k, o)| (*k, o.map(String::from))).collect()
    }

    type Case = (&'static str, &'static [(GuardKind, Option<&'static str>)]);

    #[test]
    fn flags_dangerous_statements() {
        use GuardKind::*;
        let cases: &[Case] = &[
            (
                "DELETE FROM dbo.Pedidos",
                &[(DeleteWithoutWhere, Some("dbo.Pedidos"))],
            ),
            ("delete from t", &[(DeleteWithoutWhere, Some("t"))]),
            ("DELETE t", &[(DeleteWithoutWhere, Some("t"))]),
            (
                "UPDATE t SET a = (SELECT b FROM u WHERE u.id = 1)",
                &[(UpdateWithoutWhere, Some("t"))],
            ),
            (
                "DELETE FROM t\nSELECT * FROM t WHERE 1 = 1",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            (
                "WITH c AS (SELECT * FROM t) DELETE FROM c",
                &[(DeleteWithoutWhere, Some("c"))],
            ),
            (
                "BEGIN TRY\n  DELETE FROM t\nEND TRY\nBEGIN CATCH\nEND CATCH",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            (
                "SELECT 1\nGO\nTRUNCATE TABLE dbo.Log",
                &[(Truncate, Some("dbo.Log"))],
            ),
            (
                "DROP TABLE [Sales].[Order Lines]",
                &[(Drop, Some("[Sales].[Order Lines]"))],
            ),
            ("DROP INDEX ix ON t", &[(Drop, Some("ix"))]),
            ("ALTER TABLE t DROP COLUMN c", &[(Drop, Some("t"))]),
            ("EXEC('DELETE FROM t')", &[(DeleteWithoutWhere, Some("t"))]),
            (
                "EXEC sp_executesql N'UPDATE t SET a = 1'",
                &[(UpdateWithoutWhere, Some("t"))],
            ),
            ("SELECT 'unclosed\nDROP TABLE t", &[(Unparsed, None)]),
            (
                "UPDATE TOP (10) t SET a = 1",
                &[(UpdateWithoutWhere, Some("t"))],
            ),
            (
                "DELETE FROM a; DELETE FROM b WHERE x = 1; DELETE FROM c",
                &[
                    (DeleteWithoutWhere, Some("a")),
                    (DeleteWithoutWhere, Some("c")),
                ],
            ),
            // Extra cases beyond the table above.
            (
                "UPDATE t SET a = CASE WHEN b = 1 THEN 2 ELSE 3 END WHERE id = 1; DELETE FROM u",
                &[(DeleteWithoutWhere, Some("u"))],
            ),
            (
                "UPDATE t SET a = CASE WHEN b = 1 THEN 2 ELSE 3 END",
                &[(UpdateWithoutWhere, Some("t"))],
            ),
            (
                "EXEC sys.sp_executesql N'DELETE ' + N'FROM t'",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            ("EXEC('EXEC(''DELETE FROM t'')')", &[]),
        ];
        for (sql, w) in cases {
            assert_eq!(kinds(sql), want(w), "{sql}");
        }
    }

    #[test]
    fn leaves_normal_statements_alone() {
        for sql in [
            "UPDATE t SET a = 1 WHERE id IN (SELECT id FROM u)",
            "DELETE FROM t\n  WHERE id = 4",
            "CREATE TABLE t (id int REFERENCES u ON DELETE CASCADE ON UPDATE CASCADE)",
            "CREATE TRIGGER tr ON t AFTER UPDATE, DELETE AS IF UPDATE(a) PRINT 1",
            "CREATE PROCEDURE p AS DELETE FROM t",
            "CREATE OR ALTER VIEW v AS SELECT 1 AS x",
            "GRANT SELECT, UPDATE, DELETE ON t TO r",
            "UPDATE STATISTICS dbo.t",
            "SELECT 'DELETE FROM t' AS s -- DROP TABLE t\n/* TRUNCATE TABLE t */",
            "SELECT [delete], [drop] FROM [update]",
            "DELETE FROM t OUTPUT deleted.* WHERE id = 1",
            "DROP TABLE IF EXISTS #tmp",
            "TRUNCATE TABLE #tmp; DELETE FROM @t; UPDATE ##g SET a = 1",
            "ALTER TABLE t DROP CONSTRAINT fk_a",
            "MERGE t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = s.a WHEN NOT MATCHED BY SOURCE THEN DELETE;",
            "EXEC(@sql)",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
    }

    #[test]
    fn reports_lines_relative_to_the_text() {
        let f = findings("SELECT 1\nGO\n\n  DELETE FROM t");
        assert_eq!(f[0].line, 3);
        assert_eq!(f[0].statement, "DELETE FROM t");
    }
}
