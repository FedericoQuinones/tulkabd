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
            i = number_end(b, i);
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

/// End of the numeric literal at `i`: digits, an optional fraction and
/// exponent, or hex digits after `0x`. A keyword glued to the number, as in
/// `1DROP`, starts a new token.
fn number_end(b: &[u8], mut i: usize) -> usize {
    if b[i] == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X')) {
        i += 2;
        while i < b.len() && b[i].is_ascii_hexdigit() {
            i += 1;
        }
        return i;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if b.get(j).is_some_and(u8::is_ascii_digit) {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    i
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

    /// Skips an optional `TOP (n) [PERCENT] [WITH TIES]` at `i`.
    fn skip_top(&self, i: usize) -> usize {
        if !self.is_kw(i, "TOP") {
            return i;
        }
        let mut j = i + 2;
        if self.is_punct(i + 1, '(') {
            let mut depth = 0;
            j = i + 1;
            while j < self.toks.len() {
                if self.is_punct(j, '(') {
                    depth += 1;
                } else if self.is_punct(j, ')') {
                    depth -= 1;
                    if depth == 0 {
                        j += 1;
                        break;
                    }
                }
                j += 1;
            }
        }
        if self.is_kw(j, "PERCENT") {
            j += 1;
        }
        if self.is_kw(j, "WITH") && self.is_kw(j + 1, "TIES") {
            j += 2;
        }
        j
    }

    /// Whether an `ALTER TABLE ... DROP` item list starting at `drop` removes
    /// a column. Dropping constraints, indexes or the period does not; only a
    /// later `COLUMN` item in the same list does.
    fn drops_column(&self, drop: usize) -> bool {
        let first = drop + 1;
        if self.is_kw(first, "COLUMN") {
            return true;
        }
        if !self.is_any_kw(first, &["CONSTRAINT", "INDEX", "PERIOD"]) {
            // A bare column name.
            return true;
        }
        let (end, _) = self.statement_end(drop, false);
        let mut depth = 0i32;
        for j in first..end {
            if self.is_punct(j, '(') {
                depth += 1;
            } else if self.is_punct(j, ')') {
                depth -= 1;
            } else if depth == 0 && self.is_punct(j, ',') && self.is_kw(j + 1, "COLUMN") {
                return true;
            }
        }
        false
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
                if depth == 0 {
                    // Closes a parenthesis opened before the statement.
                    return (j, where_seen);
                }
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
                    } else if self.is_kw(j, "IF")
                        && self.is_kw(j + 1, "EXISTS")
                        && !self.is_punct(j + 2, '(')
                    {
                        // `IF EXISTS` of a DROP; `IF EXISTS (...)` is a condition.
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
    // The latest `ALTER TABLE`: where it starts, the table it names and the
    // token after the table name. A `DROP` there is an action of that
    // statement; anywhere else it starts a statement of its own.
    let mut alter_table: Option<(usize, Option<String>, usize)> = None;
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
        if sc.toks[i].kind != Kind::Word {
            i += 1;
            continue;
        }
        // A data-changing statement can be composed inside parentheses:
        // `SELECT ... FROM (DELETE ... OUTPUT ...) AS d`.
        let composed = depth > 0
            && sc.is_punct(i.wrapping_sub(1), '(')
            && (sc.is_kw(i, "UPDATE") || sc.is_kw(i, "DELETE"));
        if depth > 0 && !composed {
            i += 1;
            continue;
        }

        if sc.is_kw(i, "ALTER") && sc.is_kw(i + 1, "TABLE") {
            let named = sc.name(i + 2);
            let after = named.as_ref().map_or(i + 2, |n| n.2);
            alter_table = Some((i, named.map(|n| n.0), after));
            i += 2;
            continue;
        }

        if sc.is_kw(i, "EXEC") || sc.is_kw(i, "EXECUTE") {
            if let Some(inner) = dynamic_sql(&sc, i + 1) {
                let line = base_line + sc.toks[i].line;
                if level < MAX_DYNAMIC_LEVEL {
                    let mut found = Vec::new();
                    analyze_batch(&inner, 0, level + 1, &mut found);
                    for mut f in found {
                        f.line = line;
                        out.push(f);
                    }
                } else if first_dangerous_word(&inner).is_some() {
                    // Too deep to analyze, but not safe to ignore.
                    out.push(GuardFinding {
                        kind: GuardKind::Unparsed,
                        object: None,
                        statement: truncate_statement(&inner),
                        line,
                    });
                }
            }
            i += 1;
            continue;
        }

        let is_dml = sc.is_kw(i, "UPDATE") || sc.is_kw(i, "DELETE");
        let is_start = is_dml || sc.is_kw(i, "TRUNCATE") || sc.is_kw(i, "DROP");
        if !is_start {
            i += 1;
            continue;
        }

        // `ON DELETE CASCADE` and friends are foreign key actions, but a bare
        // `ON` ends statements like `SET NOCOUNT ON`.
        let fk_action = sc.is_kw(i.wrapping_sub(1), "ON")
            && (sc.is_any_kw(i + 1, &["CASCADE", "NO"]) || sc.is_kw(i + 1, "SET"));
        let prev_blocks = i > 0
            && (fk_action
                || sc.is_any_kw(
                    i - 1,
                    &[
                        "FOR", "AFTER", "BEFORE", "OF", "GRANT", "DENY", "REVOKE", "THEN",
                    ],
                )
                || sc.is_punct(i - 1, ','));
        if is_dml && prev_blocks {
            i += 1;
            continue;
        }
        if sc.is_kw(i, "UPDATE") && (sc.is_punct(i + 1, '(') || sc.is_kw(i + 1, "STATISTICS")) {
            i += 1;
            continue;
        }
        // `ALTER ROLE r DROP MEMBER m`, `ALTER COLUMN c DROP MASKED` and the
        // like are not statements of their own.
        if sc.is_kw(i, "DROP")
            && sc.is_any_kw(
                i + 1,
                &[
                    "MEMBER",
                    "MASKED",
                    "ROWGUIDCOL",
                    "PERSISTED",
                    "HIDDEN",
                    "SPARSE",
                    "NOT",
                ],
            )
        {
            i += 1;
            continue;
        }

        // The object, and whether it is a temporary one nobody will miss.
        let (kind, object, temp, start) = if sc.is_kw(i, "UPDATE") {
            let n = sc.name(sc.skip_top(i + 1));
            let temp = n.as_ref().is_some_and(|n| n.1);
            (GuardKind::UpdateWithoutWhere, n.map(|n| n.0), temp, i)
        } else if sc.is_kw(i, "DELETE") {
            let mut j = sc.skip_top(i + 1);
            if sc.is_kw(j, "FROM") {
                j += 1;
            }
            let n = sc.name(j);
            let temp = n.as_ref().is_some_and(|n| n.1);
            (GuardKind::DeleteWithoutWhere, n.map(|n| n.0), temp, i)
        } else if sc.is_kw(i, "TRUNCATE") {
            let j = if sc.is_kw(i + 1, "TABLE") {
                i + 2
            } else {
                i + 1
            };
            let n = sc.name(j);
            let temp = n.as_ref().is_some_and(|n| n.1);
            (GuardKind::Truncate, n.map(|n| n.0), temp, i)
        } else if let Some((at, table, _)) = alter_table.clone().filter(|(_, _, after)| *after == i)
        {
            // An action of `ALTER TABLE`: only dropping a column loses data.
            if !sc.drops_column(i) {
                i += 1;
                continue;
            }
            (GuardKind::Drop, table, false, at)
        } else {
            let mut j = i + 2;
            if sc.is_kw(j, "IF") && sc.is_kw(j + 1, "EXISTS") {
                j += 2;
            }
            // `DROP TABLE a, b` drops every name in the list.
            let mut first = None;
            let mut unknown = false;
            loop {
                match sc.name(j) {
                    Some((n, temp, next)) => {
                        if !temp && first.is_none() {
                            first = Some(n);
                        }
                        j = next;
                        if sc.is_punct(j, ',') {
                            j += 1;
                            continue;
                        }
                    }
                    None => unknown = true,
                }
                break;
            }
            let all_temp = first.is_none() && !unknown;
            (GuardKind::Drop, first, all_temp, i)
        };

        if temp {
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
                object,
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
    let mut j = i;
    let parens = sc.is_punct(j, '(');
    if parens {
        j += 1;
    } else {
        // `EXEC @rc = [schema.]sp_executesql [@stmt =] N'...'`
        if sc.toks.get(j).is_some_and(|t| t.kind == Kind::Var) && sc.is_punct(j + 1, '=') {
            j += 2;
        }
        let (name, _, next) = sc.name(j)?;
        let last = name.rsplit('.').next().unwrap_or("");
        if !last
            .trim_matches(['[', ']'])
            .eq_ignore_ascii_case("sp_executesql")
        {
            return None;
        }
        j = next;
        if sc.toks.get(j).is_some_and(|t| t.kind == Kind::Var)
            && sc.src(j).eq_ignore_ascii_case("@stmt")
            && sc.is_punct(j + 1, '=')
        {
            j += 2;
        }
    }
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
                "SET NOCOUNT ON\nDELETE FROM dbo.Pedidos",
                &[(DeleteWithoutWhere, Some("dbo.Pedidos"))],
            ),
            (
                "SET XACT_ABORT ON\nDELETE FROM t",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            (
                "SET STATISTICS IO ON\nDELETE FROM t",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            (
                "ALTER DATABASE db SET READ_COMMITTED_SNAPSHOT ON\nDELETE FROM t",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            (
                "EXEC sp_executesql N'SET NOCOUNT ON DELETE FROM t'",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            (
                "ALTER TABLE t ADD c int\nDROP INDEX ix ON u",
                &[(Drop, Some("ix"))],
            ),
            (
                "ALTER TABLE t ADD c int\nDROP TABLE u",
                &[(Drop, Some("u"))],
            ),
            ("DROP TABLE #a, dbo.Real", &[(Drop, Some("dbo.Real"))]),
            (
                "DROP TABLE IF EXISTS #a, dbo.Real",
                &[(Drop, Some("dbo.Real"))],
            ),
            (
                "ALTER TABLE t DROP CONSTRAINT df_c, COLUMN c",
                &[(Drop, Some("t"))],
            ),
            (
                "EXEC sp_executesql @stmt = N'DELETE FROM t'",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            (
                "EXEC @rc = sp_executesql N'DELETE FROM t'",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            (
                "EXEC master.sys.sp_executesql N'DELETE FROM t'",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            ("EXEC('EXEC(''DELETE FROM t'')')", &[(Unparsed, None)]),
            (
                "DELETE TOP (10) PERCENT FROM t",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            (
                "DELETE TOP (10) WITH TIES FROM t",
                &[(DeleteWithoutWhere, Some("t"))],
            ),
            (
                "ALTER TABLE t DROP CONSTRAINT IF EXISTS df_c, COLUMN c",
                &[(Drop, Some("t"))],
            ),
            (
                "ALTER TABLE dbo.Pedidos DROP CONSTRAINT IF EXISTS DF_Pedidos_Estado, COLUMN IF EXISTS Estado",
                &[(Drop, Some("dbo.Pedidos"))],
            ),
            ("SELECT 1DROP TABLE t", &[(Drop, Some("t"))]),
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
            "ALTER ROLE r DROP MEMBER bob",
            "ALTER TABLE t ALTER COLUMN c DROP MASKED",
            "ALTER TABLE t ALTER COLUMN c DROP ROWGUIDCOL",
            "ALTER TABLE t ALTER COLUMN c DROP PERSISTED",
            "ALTER TABLE t ALTER COLUMN c DROP HIDDEN",
            "ALTER TABLE t ALTER COLUMN c DROP SPARSE",
            "DROP TABLE #a, #b",
            "ALTER TABLE t DROP CONSTRAINT a, b",
            "ALTER TABLE t ADD CONSTRAINT f FOREIGN KEY (a) REFERENCES u(id) ON DELETE SET NULL ON UPDATE NO ACTION",
            "CREATE SECURITY POLICY p ADD BLOCK PREDICATE f(a) ON t BEFORE DELETE",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
    }

    #[test]
    fn keeps_the_whole_statement_text() {
        for sql in [
            "DROP TABLE IF EXISTS dbo.t",
            "ALTER TABLE [dbo].[My T] DROP COLUMN IF EXISTS c",
        ] {
            let f = findings(sql);
            assert_eq!(f.len(), 1, "{sql}");
            assert_eq!(f[0].statement, sql);
        }
    }

    #[test]
    fn reports_lines_relative_to_the_text() {
        let f = findings("SELECT 1\nGO\n\n  DELETE FROM t");
        assert_eq!(f[0].line, 3);
        assert_eq!(f[0].statement, "DELETE FROM t");
    }

    #[test]
    fn flags_dml_nested_in_parentheses() {
        use GuardKind::*;
        let cases: &[Case] = &[
            (
                "INSERT INTO dbo.Log SELECT * FROM (DELETE FROM dbo.Orders OUTPUT deleted.*) AS d;",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "INSERT INTO dbo.Log (Id) SELECT d.Id FROM (UPDATE dbo.Orders SET Total = 0 OUTPUT inserted.Id) AS d WHERE d.Id > 0;",
                &[(UpdateWithoutWhere, Some("dbo.Orders"))],
            ),
        ];
        for (sql, w) in cases {
            assert_eq!(kinds(sql), want(w), "{sql}");
        }
        assert_eq!(
            kinds("INSERT INTO l SELECT * FROM (DELETE FROM o OUTPUT deleted.* WHERE id = 1) AS d"),
            vec![]
        );
    }
}
