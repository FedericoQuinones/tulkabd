//! Finds the statements of a T-SQL script that are dangerous to run
//! unattended: `UPDATE` or `DELETE` without a `WHERE`, `TRUNCATE` and `DROP`.
//!
//! This is a small lexer plus a statement scanner rather than a full parser:
//! it only has to recognise four statement shapes and where they end, and a
//! real grammar would mis-handle much of everyday T-SQL. When the lexer
//! cannot finish (an unclosed string, identifier or comment) the batch falls
//! back to a keyword search, so a dangerous word is never silently missed.
//!
//! A `MERGE` is flagged like a `DELETE` without `WHERE` when one of its
//! `WHEN ... THEN DELETE` clauses has no `AND <condition>`.
//!
//! Known limit: dynamic SQL built from variables (`EXEC(@sql)`) cannot be
//! known ahead of time.

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

/// Words that start a statement, and so end the one before them.
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
    "RECEIVE",
    "SEND",
    "GET",
    "MOVE",
    "OPEN",
    "FETCH",
    "CLOSE",
    "DEALLOCATE",
    "DENY",
    "REVOKE",
    "GOTO",
    "BREAK",
    "CONTINUE",
    "KILL",
    "DBCC",
    "CHECKPOINT",
];

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

    /// Whether `target`, a plain name in `DELETE`/`UPDATE` at `stmt..end`,
    /// is an alias of a temporary table or table variable in the `FROM`.
    fn alias_of_temporary(&self, target: &str, stmt: usize, end: usize) -> bool {
        if target.contains('.') {
            return false;
        }
        let t = target.trim_matches(['[', ']', '"']).to_lowercase();
        let named =
            |k: usize| matches!(self.toks[k].kind, Kind::Word | Kind::Quoted) && self.norm(k) == t;
        let Some(first) = (stmt + 1..end).find(|&k| named(k)) else {
            return false;
        };
        for k in first + 1..end {
            if !named(k) || self.is_punct(k + 1, '.') {
                continue;
            }
            let m = if self.is_kw(k - 1, "AS") {
                k - 2
            } else {
                k - 1
            };
            if self.is_name_part(m)
                && !self.is_any_kw(
                    m,
                    &["FROM", "JOIN", "APPLY", "ON", "SET", "WHERE", "AND", "OR"],
                )
            {
                let (start, _) = self.name_ending_at(m);
                let source_start = start > 0
                    && (self.is_any_kw(start - 1, &["FROM", "JOIN", "APPLY"])
                        || self.is_punct(start - 1, ',')
                        || self.is_punct(start - 1, '('));
                if source_start {
                    return matches!(self.toks[start].kind, Kind::Temp | Kind::Var);
                }
            }
        }
        false
    }

    /// The `(` that matches the `)` at `close`.
    fn matching_open(&self, close: usize) -> Option<usize> {
        let mut depth = 0i32;
        for j in (0..=close).rev() {
            if self.is_punct(j, ')') {
                depth += 1;
            } else if self.is_punct(j, '(') {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
        }
        None
    }

    /// The name at token `k` without its delimiters, lowercased (Unicode
    /// aware), for comparing identifiers.
    fn norm(&self, k: usize) -> String {
        let src = self.src(k);
        let src = if self.toks[k].kind == Kind::Quoted {
            src.trim_matches(['[', ']', '"'])
        } else {
            src
        };
        src.to_lowercase()
    }

    /// The dotted name that ends at token `m`: where it starts and its text.
    fn name_ending_at(&self, m: usize) -> (usize, String) {
        let mut s = m;
        loop {
            if s >= 2 && self.is_punct(s - 1, '.') && self.is_name_part(s - 2) {
                s -= 2;
            } else if s >= 3
                && self.is_punct(s - 1, '.')
                && self.is_punct(s - 2, '.')
                && self.is_name_part(s - 3)
            {
                s -= 3;
            } else {
                break;
            }
        }
        (
            s,
            self.text[self.toks[s].start..self.toks[m].end].to_string(),
        )
    }

    /// The first real (non-temporary) table named as a source in `from..to`:
    /// after `FROM`, `JOIN`, `APPLY`, or after a `,` once a `FROM` was seen.
    fn first_real_source(&self, from: usize, to: usize) -> Option<String> {
        let mut in_from = false;
        for j in from..to {
            let after_from = self.is_any_kw(j, &["FROM", "JOIN", "APPLY"]);
            if after_from {
                in_from = true;
            }
            if after_from || (in_from && self.is_punct(j, ',')) {
                if let Some((name, false, _)) = self.name(j + 1) {
                    if matches!(self.toks[j + 1].kind, Kind::Word | Kind::Quoted) {
                        return Some(name);
                    }
                }
            }
        }
        None
    }

    /// The real table that the temporary-looking `target` of the statement
    /// at `stmt..end` stands for, if any. A `#name` is only a temporary
    /// table when nothing says otherwise: a CTE of that name, or an alias of
    /// that name in the statement, makes it a real table (or whatever the
    /// alias covers). Returns the table's name, or `target` when the source
    /// cannot be named.
    fn real_table_behind(&self, target: &str, stmt: usize, end: usize) -> Option<String> {
        let t = target.trim_matches(['[', ']', '"']).to_lowercase();
        // A CTE named like the target, in the `WITH` list that comes right
        // before the statement: `WITH #c [(cols)] AS (...) DELETE ...`.
        if stmt > 0 && self.is_punct(stmt - 1, ')') {
            let mut close = stmt - 1;
            while let Some(open) = self.matching_open(close) {
                if open < 2 || !self.is_kw(open - 1, "AS") {
                    break;
                }
                let mut n = open - 2;
                if self.is_punct(n, ')') {
                    match self.matching_open(n) {
                        Some(cols) if cols > 0 => n = cols - 1,
                        _ => break,
                    }
                }
                if self.toks[n].kind != Kind::Punct && self.norm(n) == t {
                    return Some(
                        self.first_real_source(open + 1, close)
                            .unwrap_or_else(|| target.to_string()),
                    );
                }
                if n >= 2 && self.is_punct(n - 1, ',') && self.is_punct(n - 2, ')') {
                    close = n - 2;
                } else {
                    break;
                }
            }
        }
        // An alias: the first occurrence is the target itself; any later one
        // that is not a table reference (after FROM, JOIN, APPLY, `,` or
        // `(`) and not a qualifier (`#o.col`) names something else's alias.
        let first =
            (stmt + 1..end).find(|&k| self.toks[k].kind != Kind::Punct && self.norm(k) == t)?;
        for k in first + 1..end {
            if self.toks[k].kind == Kind::Punct || self.norm(k) != t || self.is_punct(k + 1, '.') {
                continue;
            }
            let prev = k - 1;
            if self.is_any_kw(prev, &["FROM", "JOIN", "APPLY"])
                || self.is_punct(prev, ',')
                || self.is_punct(prev, '(')
            {
                continue;
            }
            let m = if self.is_kw(prev, "AS") {
                prev - 1
            } else {
                prev
            };
            if self.is_punct(m, ')') {
                // `(subquery) #o`, `fn() #o`, `dbo.t (TABLOCK) #o`.
                let mut depth = 0i32;
                let mut open = m;
                for j in (0..=m).rev() {
                    if self.is_punct(j, ')') {
                        depth += 1;
                    } else if self.is_punct(j, '(') {
                        depth -= 1;
                        if depth == 0 {
                            open = j;
                            break;
                        }
                    }
                }
                if open > 0
                    && self.is_name_part(open - 1)
                    && !self.is_any_kw(open - 1, &["FROM", "JOIN", "APPLY"])
                {
                    let (start, name) = self.name_ending_at(open - 1);
                    if matches!(self.toks[start].kind, Kind::Word | Kind::Quoted) {
                        return Some(name);
                    }
                    continue;
                }
                return Some(
                    self.first_real_source(open, m)
                        .unwrap_or_else(|| target.to_string()),
                );
            }
            const NOT_ALIAS_OF: &[&str] = &[
                "INTO", "ON", "SET", "WHERE", "AND", "OR", "NOT", "IN", "BY", "UPDATE", "DELETE",
                "INSERT", "TABLE", "VALUES", "SELECT", "TOP", "EXISTS", "OUTPUT", "HAVING", "WHEN",
                "THEN", "ELSE", "CASE", "AS",
            ];
            if self.is_name_part(m) && !self.is_any_kw(m, NOT_ALIAS_OF) {
                let (start, name) = self.name_ending_at(m);
                if matches!(self.toks[start].kind, Kind::Word | Kind::Quoted) {
                    return Some(name);
                }
            }
        }
        None
    }

    /// Start of the `+` expression that contains the token at `e`.
    fn expr_start(&self, mut e: usize) -> usize {
        while e >= 2 && self.is_punct(e - 1, '+') {
            let l = e - 2;
            let mut start = l;
            if self.is_punct(l, ')') {
                let mut depth = 0i32;
                for j in (0..=l).rev() {
                    if self.is_punct(j, ')') {
                        depth += 1;
                    } else if self.is_punct(j, '(') {
                        depth -= 1;
                        if depth == 0 {
                            start = if j > 0 && self.is_name_part(j - 1) {
                                j - 1
                            } else {
                                j
                            };
                            break;
                        }
                    }
                }
            }
            e = start;
        }
        e
    }

    /// The text of the `+` expression that starts at `start`: literals
    /// joined, anything else (variables, function calls) as a ` __v ` gap.
    /// Also whether it is made of literals only.
    fn expr_text(&self, start: usize) -> (String, bool) {
        let mut text = String::new();
        let mut pure = true;
        let mut j = start;
        while j < self.toks.len() {
            if self.toks[j].kind == Kind::Str {
                text.push_str(&unquote(self.src(j)));
                j += 1;
            } else {
                pure = false;
                text.push_str(" __v ");
                j = self.skip_operand(j);
            }
            if self.is_punct(j, '+') {
                j += 1;
            } else {
                break;
            }
        }
        (text, pure)
    }

    /// The `(` of the `CONCAT(` call that directly encloses token `k`.
    fn concat_open(&self, k: usize) -> Option<usize> {
        let mut depth = 0i32;
        for j in (0..k).rev() {
            if self.is_punct(j, ')') {
                depth += 1;
            } else if self.is_punct(j, '(') {
                if depth == 0 {
                    return (j > 0 && self.is_kw(j - 1, "CONCAT")).then_some(j);
                }
                depth -= 1;
            }
        }
        None
    }

    /// Like [`Scan::expr_text`] for the arguments of the `CONCAT(` at `open`:
    /// they are parts of one expression.
    fn concat_text(&self, open: usize) -> (String, bool) {
        let mut close = open;
        let mut depth = 0i32;
        while close < self.toks.len() {
            if self.is_punct(close, '(') {
                depth += 1;
            } else if self.is_punct(close, ')') {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            close += 1;
        }
        let mut text = String::new();
        let mut pure = true;
        let mut a = open + 1;
        while a < close {
            // The argument ends at the next top-level comma.
            let mut b = a;
            let mut d = 0i32;
            while b < close {
                if self.is_punct(b, '(') {
                    d += 1;
                } else if self.is_punct(b, ')') {
                    d -= 1;
                } else if d == 0 && self.is_punct(b, ',') {
                    break;
                }
                b += 1;
            }
            // Only literals joined by `+` are text; a `CASE`, a call or a
            // parenthesized argument can hide what it adds.
            let direct = (b - a) % 2 == 1
                && (a..b).all(|k| {
                    if (k - a).is_multiple_of(2) {
                        self.toks[k].kind == Kind::Str
                    } else {
                        self.is_punct(k, '+')
                    }
                });
            if direct {
                for k in (a..b).step_by(2) {
                    text.push_str(&unquote(self.src(k)));
                }
            } else {
                pure = false;
                text.push_str(" __v ");
            }
            a = b + 1;
        }
        (text, pure)
    }

    /// The token after the non-literal operand that starts at `j`: a
    /// parenthesized group, a `CASE ... END`, or a name with call parentheses.
    fn skip_operand(&self, mut j: usize) -> usize {
        let skip_group = |mut j: usize| {
            let mut depth = 0i32;
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
            j
        };
        if self.is_punct(j, '(') {
            return skip_group(j);
        }
        if self.is_kw(j, "CASE") {
            let mut cases = 0i32;
            while j < self.toks.len() {
                if self.is_kw(j, "CASE") {
                    cases += 1;
                } else if self.is_kw(j, "END") {
                    cases -= 1;
                    if cases == 0 {
                        return j + 1;
                    }
                }
                j += 1;
            }
            return j;
        }
        j += 1;
        while self.is_punct(j, '(') {
            j = skip_group(j);
        }
        j
    }

    /// Whether the literal at `start` is an argument of `PRINT`, `RAISERROR`
    /// or `THROW`: a message, not code.
    fn in_message_context(&self, start: usize) -> bool {
        for j in (0..start).rev() {
            if self.is_punct(j, ';') || self.is_kw(j, "SET") {
                return false;
            }
            if self.is_any_kw(j, &["PRINT", "RAISERROR", "THROW"]) {
                return true;
            }
            if self.is_any_kw(j, OPENERS) {
                return false;
            }
        }
        false
    }

    /// Whether the opener word at `j` begins a statement here: some of them
    /// are also ordinary column or alias names.
    fn starts_statement(&self, j: usize) -> bool {
        if self.is_kw(j, "SEND") {
            return self.is_kw(j + 1, "ON");
        }
        if self.is_any_kw(j, &["GET", "MOVE"]) {
            return self.is_kw(j + 1, "CONVERSATION");
        }
        if self.is_any_kw(j, &["RECEIVE", "THROW"]) {
            let prev_blocks = j > 0
                && (self.is_any_kw(j - 1, &["SET", "AS"])
                    || (self.toks[j - 1].kind == Kind::Punct
                        && !self.is_punct(j - 1, ';')
                        && !self.is_punct(j - 1, ':')
                        && !self.is_punct(j - 1, ')')));
            // `JOIN dbo.Log Throw ON ...`: followed by what follows a table alias.
            let next_blocks = self.is_any_kw(
                j + 1,
                &[
                    "ON", "JOIN", "CROSS", "INNER", "LEFT", "RIGHT", "FULL", "OUTER", "WITH",
                    "WHERE", "SET", "GROUP", "ORDER",
                ],
            ) || [',', '.', ')', '=']
                .iter()
                .any(|&c| self.is_punct(j + 1, c));
            return !prev_blocks && !next_blocks;
        }
        true
    }

    /// Finds where the `MERGE` at `from` ends (its `;`, the word that opens
    /// the next statement or the token count) and whether one of its
    /// `WHEN ... THEN DELETE` clauses has no `AND <condition>`.
    fn merge_end(&self, from: usize) -> (usize, bool) {
        let mut depth = 0i32;
        let mut deletes_all = false;
        // Open CASE expressions: their WHEN, THEN, ELSE and END are not clauses.
        let mut cases = 0u32;
        let mut j = from + 1;
        while j < self.toks.len() {
            if self.is_punct(j, '(') {
                depth += 1;
            } else if self.is_punct(j, ')') {
                if depth == 0 {
                    return (j, deletes_all);
                }
                depth -= 1;
            } else if depth == 0 {
                if self.is_punct(j, ';') {
                    return (j, deletes_all);
                }
                if self.is_kw(j, "CASE") {
                    cases += 1;
                } else if cases > 0 {
                    if self.is_kw(j, "END") {
                        cases -= 1;
                    }
                } else if self.is_kw(j, "WHEN") {
                    // The clause runs to its THEN; an AND in between is its condition.
                    let mut k = j + 1;
                    let mut inner = 0i32;
                    let mut inner_cases = 0u32;
                    let mut conditioned = false;
                    while k < self.toks.len() {
                        if self.is_punct(k, '(') {
                            inner += 1;
                        } else if self.is_punct(k, ')') {
                            inner -= 1;
                        } else if inner == 0 && self.is_kw(k, "CASE") {
                            inner_cases += 1;
                        } else if inner == 0 && inner_cases > 0 {
                            if self.is_kw(k, "END") {
                                inner_cases -= 1;
                            }
                        } else if inner == 0 && self.is_kw(k, "THEN") {
                            break;
                        } else if inner == 0 && self.is_kw(k, "AND") {
                            conditioned = true;
                        }
                        k += 1;
                    }
                    if self.is_kw(k + 1, "DELETE") && !conditioned {
                        deletes_all = true;
                    }
                    j = k;
                } else if self.toks[j].kind == Kind::Word
                    && self.is_any_kw(j, OPENERS)
                    && !self.is_any_kw(j - 1, &["THEN", "AS"])
                    && !self.is_punct(j + 1, '.')
                    && self.starts_statement(j)
                {
                    return (j, deletes_all);
                }
            }
            j += 1;
        }
        (self.toks.len(), deletes_all)
    }

    /// Finds where the statement that starts at `from` ends: the index of the
    /// `;` or of the word that opens the next statement, or the token count.
    /// The statement is an `UPDATE` when `is_update`, whose first `SET` is
    /// part of it. Also reports whether a top-level `WHERE` was seen.
    fn statement_end(&self, from: usize, is_update: bool) -> (usize, bool) {
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
                    } else if self.is_any_kw(j, OPENERS) && self.starts_statement(j) {
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

    // A batch may start with the procedure name and no `EXEC`.
    if sc
        .toks
        .first()
        .is_some_and(|t| matches!(t.kind, Kind::Word | Kind::Quoted))
    {
        dynamic_call(&sc, 0, true, base_line + sc.toks[0].line, level, out);
    }

    // `DECLARE @s = N'DELETE ...'; EXEC(@s)`: the executed text is out of
    // reach, but the literals of a batch that runs a variable may be it, so
    // each is analyzed as if it were executed.
    let mut runs_variable = false;
    // Token ranges of the arguments of dynamic calls, reported by the call.
    let mut call_args_ranges: Vec<(usize, usize)> = Vec::new();
    for i in 0..sc.toks.len() {
        let arg = if i == 0 && starts_like_a_call(&sc) {
            i
        } else if sc.is_kw(i, "EXEC") || sc.is_kw(i, "EXECUTE") {
            i + 1
        } else {
            continue;
        };
        let Some((j, _)) = call_args(&sc, arg) else {
            continue;
        };
        let (e, _) = sc.statement_end(j.saturating_sub(1), false);
        call_args_ranges.push((j, e));
        if dynamic_sql(&sc, arg).is_some() {
            continue;
        }
        // Does the first argument hold a variable anywhere in its expression?
        let mut depth = 0i32;
        for k in j..e {
            if sc.is_punct(k, '(') {
                depth += 1;
            } else if sc.is_punct(k, ')') {
                depth -= 1;
            } else if depth == 0 && sc.is_punct(k, ',') {
                break;
            } else if sc.toks[k].kind == Kind::Var {
                runs_variable = true;
            }
        }
    }
    if runs_variable {
        let other_literal_has_set = sc
            .toks
            .iter()
            .enumerate()
            .any(|(k, t)| t.kind == Kind::Str && has_word(&unquote(sc.src(k)), "SET"));
        // A header such as `N'MERGE dbo.Orders AS tgt'` is told from the word
        // in prose by a USING or INTO in some literal of the batch.
        let other_literal_has_merge_part = sc.toks.iter().enumerate().any(|(k, t)| {
            t.kind == Kind::Str && {
                let lit = unquote(sc.src(k));
                has_word(&lit, "USING") || has_word(&lit, "INTO")
            }
        });
        let mut seen = Vec::new();
        for k in 0..sc.toks.len() {
            if sc.toks[k].kind != Kind::Str {
                continue;
            }
            let concat = sc.concat_open(k);
            let start = concat.unwrap_or_else(|| sc.expr_start(k));
            if seen.contains(&start) {
                continue;
            }
            seen.push(start);
            if call_args_ranges
                .iter()
                .any(|&(a, b)| start >= a && start < b)
                || sc.in_message_context(start)
            {
                continue;
            }
            let (text, pure) = match concat {
                Some(open) => sc.concat_text(open),
                None => sc.expr_text(start),
            };
            let line = base_line + sc.toks[start].line;
            let unparsed = || GuardFinding {
                kind: GuardKind::Unparsed,
                object: None,
                statement: truncate_statement(&text),
                line,
            };
            let first_word = text
                .trim_start()
                .split(|c: char| !is_word_char(c))
                .next()
                .unwrap_or("");
            if first_word.eq_ignore_ascii_case("MERGE")
                && (!pure
                    || (other_literal_has_merge_part
                        && !has_word(&text, "USING")
                        && !has_word(&text, "INTO")))
            {
                // A MERGE with holes in it, or only a header whose USING is in
                // another literal, cannot be called safe.
                out.push(unparsed());
            } else if level < MAX_DYNAMIC_LEVEL {
                let mut found = Vec::new();
                analyze_batch(&text, 0, level + 1, &mut found);
                for mut f in found {
                    // `N'Update finished'` is prose; a lone UPDATE counts
                    // only with a SET here or in another literal.
                    if pure
                        && f.kind == GuardKind::UpdateWithoutWhere
                        && !has_word(&f.statement, "SET")
                        && !other_literal_has_set
                    {
                        continue;
                    }
                    f.line = line;
                    if !pure {
                        // Built in pieces: what runs is not known, only that
                        // the statement it starts has no bounding WHERE.
                        f.kind = GuardKind::Unparsed;
                        f.object = None;
                    }
                    out.push(f);
                }
            } else if first_dangerous_word(&text).is_some() {
                out.push(unparsed());
            }
        }
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
            && (sc.is_kw(i, "UPDATE") || sc.is_kw(i, "DELETE") || sc.is_kw(i, "MERGE"));
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
            dynamic_call(&sc, i + 1, false, base_line + sc.toks[i].line, level, out);
            i += 1;
            continue;
        }

        // `MERGE ... WHEN MATCHED THEN DELETE` with no condition can empty the
        // target. The MERGE is not skipped, so statements nested in it are still read.
        if sc.is_kw(i, "MERGE") && !sc.is_kw(i + 1, "JOIN") {
            let (end, deletes_all) = sc.merge_end(i);
            let mut j = sc.skip_top(i + 1);
            if sc.is_kw(j, "INTO") {
                j += 1;
            }
            let target = sc.name(j);
            if deletes_all && !target.as_ref().is_some_and(|n| n.1) {
                let last = end.saturating_sub(1).max(i);
                out.push(GuardFinding {
                    kind: GuardKind::DeleteWithoutWhere,
                    object: target.map(|n| n.0),
                    statement: truncate_statement(&text[sc.toks[i].start..sc.toks[last].end]),
                    line: base_line + sc.toks[i].line,
                });
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
            && (sc.is_kw(i + 1, "CASCADE")
                || (sc.is_kw(i + 1, "NO") && sc.is_kw(i + 2, "ACTION"))
                || sc.is_kw(i + 1, "SET"));
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
        let (kind, object, mut temp, start) = if sc.is_kw(i, "UPDATE") {
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

        if !temp && is_dml {
            // `DELETE t FROM #t t`: the alias stands for a temporary table.
            if let Some(target) = &object {
                let (end, _) = sc.statement_end(i, sc.is_kw(i, "UPDATE"));
                temp = sc.alias_of_temporary(target, i, end);
            }
        }
        let mut object = object;
        if temp && is_dml {
            // A `#name` can stand for a real table: an alias in the FROM
            // clause (`DELETE #o FROM dbo.Orders #o`) or a CTE.
            let (end, _) = sc.statement_end(i, sc.is_kw(i, "UPDATE"));
            let target = object.clone().unwrap_or_default();
            if let Some(real) = sc.real_table_behind(&target, i, end) {
                temp = false;
                object = Some(real);
            }
        }
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

/// Analyzes the literal statement of the dynamic call whose name or `(`
/// is at token `arg`, if there is one. Findings are reported at `line`.
fn dynamic_call(
    sc: &Scan,
    arg: usize,
    implicit: bool,
    line: u32,
    level: u8,
    out: &mut Vec<GuardFinding>,
) {
    if let Some(inner) = dynamic_sql(sc, arg) {
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
    } else if !implicit || starts_like_a_call(sc) {
        // Not a plain literal, so it cannot be analyzed; if the arguments
        // carry a dangerous word, say so instead of staying silent.
        let (end, _) = sc.statement_end(arg, false);
        // A named procedure taking `N'Update totals'` is far more often a
        // message than a statement: its UPDATE needs a SET to count.
        let procedure = !sc.is_punct(arg, '(') && call_args(sc, arg).is_none();
        let hit = literal_chains(sc, arg, end)
            .iter()
            .any(|(_, text)| has_dangerous_word(text, procedure));
        if hit {
            out.push(GuardFinding {
                kind: GuardKind::Unparsed,
                object: None,
                statement: truncate_statement(
                    &sc.text[sc.toks[arg].start..sc.toks[end.max(arg + 1) - 1].end],
                ),
                line,
            });
        }
    }
}

/// The string literals in tokens `from..end`, each run of literals joined by
/// `+` as one text, with the index of its first token.
fn literal_chains(sc: &Scan, from: usize, end: usize) -> Vec<(usize, String)> {
    let mut chains = Vec::new();
    let mut j = from;
    while j < end {
        if sc.toks[j].kind != Kind::Str {
            j += 1;
            continue;
        }
        let first = j;
        let mut text = unquote(sc.src(j));
        j += 1;
        while j + 1 < end && sc.is_punct(j, '+') && sc.toks[j + 1].kind == Kind::Str {
            text.push_str(&unquote(sc.src(j + 1)));
            j += 2;
        }
        chains.push((first, text));
    }
    chains
}

/// Whether the batch starts like a procedure call without `EXEC`: a name
/// followed by an argument or nothing, which keeps `BACKUP DATABASE x TO
/// DISK = '...'` and the like out.
fn starts_like_a_call(sc: &Scan) -> bool {
    if sc.is_any_kw(0, OPENERS) || sc.is_any_kw(0, &["WITH", "SET"]) {
        return false;
    }
    match sc.name(0) {
        Some((_, _, next)) => sc
            .toks
            .get(next)
            .is_none_or(|t| matches!(t.kind, Kind::Str | Kind::Var | Kind::Num)),
        None => false,
    }
}

/// Where the argument of a dynamic call starts: after the `(` of `EXEC(...)`,
/// or after the name of `sp_executesql` and an optional `@stmt =`. Also says
/// whether it is the parenthesized form. `i` is the token after `EXEC`.
fn call_args(sc: &Scan, i: usize) -> Option<(usize, bool)> {
    let mut j = i;
    if sc.is_punct(j, '(') {
        return Some((j + 1, true));
    }
    // `EXEC @rc = [schema.]sp_executesql [@stmt =] N'...'`
    if sc.toks.get(j).is_some_and(|t| t.kind == Kind::Var) && sc.is_punct(j + 1, '=') {
        j += 2;
    }
    let (name, _, next) = sc.name(j)?;
    let last = name.rsplit('.').next().unwrap_or("");
    if !last
        .trim_matches(['[', ']', '"'])
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
    Some((j, false))
}

/// The text of a literal dynamic statement: the argument of `EXEC(...)` or
/// of `sp_executesql`, when it is made only of string literals joined by `+`.
/// `i` is the token after `EXEC`.
fn dynamic_sql(sc: &Scan, i: usize) -> Option<String> {
    let (mut j, parens) = call_args(sc, i)?;
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

/// Whether any word of `text` is `DELETE`, `DROP` or `TRUNCATE`, or `UPDATE`
/// (which counts only with a `SET` when `update_needs_set`).
fn has_dangerous_word(text: &str, update_needs_set: bool) -> bool {
    text.split(|c: char| !is_word_char(c)).any(|w| {
        ["DELETE", "DROP", "TRUNCATE"]
            .iter()
            .any(|k| w.eq_ignore_ascii_case(k))
            || (w.eq_ignore_ascii_case("UPDATE") && (!update_needs_set || has_word(text, "SET")))
    })
}

/// Whether `text` has `word` as a whole word, ignoring case.
fn has_word(text: &str, word: &str) -> bool {
    text.split(|c: char| !is_word_char(c))
        .any(|w| w.eq_ignore_ascii_case(word))
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
            "MERGE t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = s.a WHEN NOT MATCHED BY SOURCE AND t.old = 1 THEN DELETE;",
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
    fn merge_deleting_without_a_condition_is_flagged() {
        let delete_t: &[(GuardKind, Option<&str>)] =
            &[(GuardKind::DeleteWithoutWhere, Some("dbo.T"))];
        for sql in [
            "MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.a = s.a WHEN NOT MATCHED BY SOURCE THEN DELETE;",
            "MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.a = s.a WHEN NOT MATCHED BY SOURCE THEN DELETE",
            "MERGE INTO dbo.T AS t\nUSING dbo.S AS s ON t.id = s.id\nWHEN MATCHED THEN DELETE;",
            "MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN NOT MATCHED BY SOURCE THEN DELETE OUTPUT $action, deleted.id;",
            "MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN NOT MATCHED BY TARGET THEN INSERT (id) VALUES (s.id) WHEN NOT MATCHED BY SOURCE THEN DELETE OUTPUT deleted.* INTO #log;",
            "WITH src AS (SELECT id FROM dbo.S) MERGE INTO dbo.T AS t USING src ON t.id = src.id WHEN NOT MATCHED BY SOURCE THEN DELETE;",
            ";WITH src AS (SELECT id FROM dbo.S)\nMERGE dbo.T AS t USING src ON t.id = src.id WHEN MATCHED THEN DELETE;",
            "MERGE TOP (10) dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED THEN DELETE;",
            "MERGE dbo.T AS t USING (SELECT id FROM dbo.S WHERE ok = 1) AS s ON t.id = s.id WHEN MATCHED AND s.id > 5 THEN UPDATE SET t.a = 1 WHEN NOT MATCHED BY SOURCE THEN DELETE;",
        ] {
            assert_eq!(kinds(sql), want(delete_t), "{sql}");
        }
        // One finding per MERGE, and statements around it are still read.
        assert_eq!(
            kinds("MERGE dbo.T t USING dbo.S s ON t.id = s.id WHEN MATCHED THEN DELETE WHEN NOT MATCHED BY SOURCE THEN DELETE; DELETE FROM dbo.U"),
            want(&[(GuardKind::DeleteWithoutWhere, Some("dbo.T")), (GuardKind::DeleteWithoutWhere, Some("dbo.U"))])
        );
        let f = findings("SELECT 1;\nMERGE dbo.T t USING dbo.S s ON t.id = s.id WHEN MATCHED THEN DELETE;\nSELECT 2;");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].line, 1);
        assert!(
            f[0].statement.starts_with("MERGE dbo.T") && f[0].statement.ends_with("DELETE"),
            "{}",
            f[0].statement
        );
    }

    #[test]
    fn merge_that_conditions_its_deletes_or_does_not_delete_is_left_alone() {
        for sql in [
            "MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED AND s.deleted = 1 THEN DELETE;",
            "MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN NOT MATCHED BY SOURCE AND t.active = 1 THEN DELETE",
            "MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED AND (s.a = 1 OR s.b = 2) THEN DELETE OUTPUT deleted.id;",
            "MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.a = s.a WHEN NOT MATCHED THEN INSERT (id) VALUES (s.id);",
            "WITH src AS (SELECT id FROM dbo.S) MERGE dbo.T AS t USING src ON t.id = src.id WHEN NOT MATCHED THEN INSERT (id) VALUES (src.id) OUTPUT inserted.id;",
            "MERGE #t AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED THEN DELETE;",
            "SELECT * FROM a INNER MERGE JOIN b ON a.id = b.id OPTION (MERGE JOIN)",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
    }

    #[test]
    fn merge_with_case_expressions_is_still_read_to_its_end() {
        for sql in [
            "MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.a = CASE WHEN s.a IS NULL THEN 0 ELSE s.a END WHEN NOT MATCHED BY SOURCE THEN DELETE;",
            "MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.a = CASE WHEN s.a IS NULL THEN 0 END WHEN NOT MATCHED BY SOURCE THEN DELETE;",
            "MERGE dbo.T AS t USING dbo.S AS s ON t.k = CASE WHEN s.k IS NULL THEN 0 ELSE s.k END WHEN MATCHED THEN DELETE;",
            "MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED AND CASE WHEN s.a IS NULL THEN 0 ELSE 1 END = 1 THEN UPDATE SET t.a = 1 WHEN NOT MATCHED BY SOURCE THEN DELETE;",
            "EXEC(N'MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED THEN UPDATE SET t.a = CASE WHEN s.a IS NULL THEN 0 ELSE s.a END WHEN NOT MATCHED BY SOURCE THEN DELETE;')",
        ] {
            assert_eq!(kinds(sql).len(), 1, "{sql}");
        }
        assert_eq!(
            kinds("MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN MATCHED AND CASE WHEN s.a IS NULL THEN 0 ELSE 1 END = 1 THEN DELETE;"),
            vec![]
        );
    }

    #[test]
    fn merge_inside_parentheses_is_read() {
        assert_eq!(
            kinds("INSERT INTO dbo.Log SELECT act, id FROM (MERGE dbo.T AS t USING dbo.S AS s ON t.id = s.id WHEN NOT MATCHED BY SOURCE THEN DELETE OUTPUT $action, deleted.id) AS c (act, id);"),
            want(&[(GuardKind::DeleteWithoutWhere, Some("dbo.T"))])
        );
    }

    #[test]
    fn a_source_alias_named_like_an_opener_does_not_end_the_merge() {
        for alias in ["send", "close", "open", "move", "get"] {
            let sql = format!("MERGE dbo.T AS t USING dbo.S AS {alias} ON t.id = {alias}.id WHEN NOT MATCHED BY SOURCE THEN DELETE;");
            assert_eq!(kinds(&sql).len(), 1, "{sql}");
        }
    }

    #[test]
    fn a_pure_merge_literal_in_a_variable_is_analyzed() {
        assert_eq!(
            kinds("DECLARE @s nvarchar(max) = N'MERGE dbo.T t USING dbo.S s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = 1;'; EXEC(@s)"),
            vec![]
        );
        assert_eq!(
            kinds("DECLARE @s nvarchar(max) = N'MERGE dbo.T t USING dbo.S s ON t.id = s.id WHEN MATCHED THEN DELETE;'; EXEC(@s)"),
            want(&[(GuardKind::DeleteWithoutWhere, Some("dbo.T"))])
        );
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

    #[test]
    fn flags_implicit_sp_executesql_calls() {
        use GuardKind::*;
        let cases: &[Case] = &[
            (
                "sp_executesql N'DELETE FROM dbo.Orders'",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "sys.sp_executesql N'TRUNCATE TABLE dbo.Orders'",
                &[(Truncate, Some("dbo.Orders"))],
            ),
            (
                "EXEC \"sp_executesql\" N'DELETE FROM dbo.Orders'",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "[sys].[sp_executesql] N'DELETE FROM dbo.Orders'",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "EXEC('sp_executesql N''DELETE FROM dbo.Orders''')",
                &[(Unparsed, None)],
            ),
        ];
        for (sql, w) in cases {
            assert_eq!(kinds(sql), want(w), "{sql}");
        }
    }

    #[test]
    fn real_tables_behind_a_hash_alias_are_flagged() {
        use GuardKind::*;
        let cases: &[Case] = &[
            (
                "DELETE #o FROM dbo.Orders AS #o",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "UPDATE #o SET Total = 0 FROM dbo.Orders #o",
                &[(UpdateWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DELETE FROM #o FROM dbo.Orders #o",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
        ];
        for (sql, w) in cases {
            assert_eq!(kinds(sql), want(w), "{sql}");
        }
        for sql in [
            "DELETE FROM #o",
            "UPDATE #o SET a = 1",
            "DELETE #o FROM #o JOIN #p ON #o.id = #p.id",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
    }

    #[test]
    fn statements_after_less_common_openers_are_separate() {
        use GuardKind::*;
        for opener in [
            "RECEIVE TOP (1) * FROM dbo.Q",
            "GOTO done",
            "KILL 55",
            "DBCC CHECKDB",
            "CHECKPOINT",
            "DEALLOCATE c",
        ] {
            let sql = format!("DELETE FROM dbo.Orders {opener} WHERE x = 1");
            assert_eq!(
                kinds(&sql),
                want(&[(DeleteWithoutWhere, Some("dbo.Orders"))]),
                "{sql}"
            );
        }
    }

    #[test]
    fn only_no_action_is_a_foreign_key_action() {
        use GuardKind::*;
        assert_eq!(
            kinds("SET NOCOUNT ON DELETE NO"),
            want(&[(DeleteWithoutWhere, Some("NO"))])
        );
        assert_eq!(
            kinds("ALTER TABLE t ADD CONSTRAINT f FOREIGN KEY (a) REFERENCES u(id) ON DELETE NO ACTION"),
            vec![]
        );
    }

    #[test]
    fn flags_calls_with_dangerous_literals_it_cannot_analyze() {
        use GuardKind::*;
        for sql in [
            "EXEC sp_MSforeachtable 'TRUNCATE TABLE ?'",
            "EXEC(N'DELETE FROM dbo.Orders' + @where)",
            "sp_MSforeachtable 'DROP TABLE ?'",
        ] {
            assert_eq!(kinds(sql), want(&[(Unparsed, None)]), "{sql}");
        }
        for sql in [
            "EXEC(@sql)",
            "EXEC dbo.usp_Log 'started'",
            "SELECT 'DELETE it' AS s",
            "SET @a = 'DELETE'",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
    }

    #[test]
    fn ambiguous_openers_only_count_in_their_statement_form() {
        use GuardKind::*;
        for sql in [
            "UPDATE dbo.Orders SET Send = 1 WHERE Id = 1",
            "UPDATE dbo.Orders SET Total=1, Receive=2 WHERE Id=1",
            "UPDATE o SET Total = o.[Total] + o.Get FROM dbo.Orders o WHERE o.Id = 1",
            "DELETE o FROM dbo.Orders o JOIN dbo.Log l ON l.Move = o.Id WHERE o.Id = 1",
            "UPDATE dbo.Orders SET Throw = 1 WHERE Id = 1",
            "UPDATE dbo.Orders SET Total = Receive + 1 WHERE Id = 1",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
        for opener in [
            "SEND ON CONVERSATION @h",
            "GET CONVERSATION GROUP @g FROM dbo.Q",
            "MOVE CONVERSATION @h TO @g",
            "THROW 50000, 'x', 1",
            "RECEIVE TOP (1) * FROM dbo.Q",
        ] {
            let sql = format!("DELETE FROM dbo.Orders {opener} WHERE x = 1");
            assert_eq!(
                kinds(&sql),
                want(&[(DeleteWithoutWhere, Some("dbo.Orders"))]),
                "{sql}"
            );
        }
    }

    #[test]
    fn only_call_shaped_batches_count_as_implicit_calls() {
        use GuardKind::*;
        for sql in [
            "BACKUP DATABASE x TO DISK = '/tmp/drop/x.bak'",
            "RESTORE DATABASE x FROM DISK = '/tmp/delete/x.bak'",
            "BULK INSERT dbo.t FROM '/tmp/truncate/x.csv'",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
        assert_eq!(
            kinds("sp_MSforeachtable 'TRUNCATE TABLE ?'"),
            want(&[(Unparsed, None)])
        );
        assert_eq!(
            kinds("sp_MSforeachtable @command1 = 'TRUNCATE TABLE ?'"),
            want(&[(Unparsed, None)])
        );
    }

    #[test]
    fn literals_joined_with_plus_are_checked_as_one() {
        use GuardKind::*;
        assert_eq!(
            kinds("EXEC(N'DEL' + N'ETE FROM dbo.Orders' + @w)"),
            want(&[(Unparsed, None)])
        );
    }

    #[test]
    fn literals_of_a_batch_that_executes_a_variable_are_analyzed() {
        use GuardKind::*;
        let cases: &[Case] = &[
            (
                "DECLARE @s nvarchar(max) = N'DELETE FROM dbo.Orders'; EXEC sp_executesql @s",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DECLARE @s nvarchar(max) = N'DELETE FROM dbo.Orders'; EXEC(@s)",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DECLARE @s nvarchar(max) = N'DELETE FROM dbo.Orders'; EXEC(N'' + @s)",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DECLARE @s nvarchar(max) = N'TRUNCATE TABLE ' + N'dbo.Orders'; EXEC sp_executesql @stmt = @s",
                &[(Truncate, Some("dbo.Orders"))],
            ),
        ];
        for (sql, w) in cases {
            assert_eq!(kinds(sql), want(w), "{sql}");
        }
        for sql in [
            "DECLARE @s nvarchar(max) = N'SELECT 1'; EXEC(@s)",
            "DECLARE @s nvarchar(max) = N'DELETE FROM dbo.Orders'; SELECT @s",
            "DECLARE @s nvarchar(max) = N'DELETE FROM dbo.Orders WHERE Id=@id'; EXEC sp_executesql @s, N'@id int', @id=1",
            "EXEC('DELETE FROM dbo.Orders WHERE Id=1'); EXEC(@s)",
            "PRINT 'Update finished'; EXEC(@sql)",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
    }

    #[test]
    fn a_hash_name_that_stands_for_a_real_table_is_flagged() {
        use GuardKind::*;
        let cases: &[Case] = &[
            (
                "UPDATE #o SET Total = 0 FROM #t JOIN dbo.Orders #o ON 1 = 1",
                &[(UpdateWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DELETE #o FROM #t, dbo.Orders #o",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DELETE #o FROM #t CROSS JOIN dbo.Orders AS #o",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DELETE #o FROM (#t JOIN dbo.Orders #o ON 1 = 1)",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "WITH #c AS (SELECT * FROM dbo.Orders) DELETE FROM #c",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "WITH #c (a) AS (SELECT Total FROM dbo.Orders) UPDATE #c SET a = 0",
                &[(UpdateWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "UPDATE #o SET Send = 1 FROM dbo.Orders #o",
                &[(UpdateWithoutWhere, Some("dbo.Orders"))],
            ),
        ];
        for (sql, w) in cases {
            assert_eq!(kinds(sql), want(w), "{sql}");
        }
        for sql in [
            "UPDATE #s SET Total = 1 FROM dbo.Orders o JOIN #s ON #s.Id = o.Id",
            "DELETE FROM #t",
            "DELETE #t FROM dbo.Orders AS x JOIN #t ON x.Id = #t.Id",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
    }

    #[test]
    fn a_hash_target_is_temporary_only_when_nothing_says_otherwise() {
        use GuardKind::*;
        let cases: &[Case] = &[
            (
                "DELETE #o FROM dbo.Orders [#o]",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DELETE #o FROM (SELECT * FROM dbo.Orders) #o",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "UPDATE #o SET Total=0 FROM (SELECT * FROM dbo.Orders) AS #o",
                &[(UpdateWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DELETE #o FROM dbo.Orders (TABLOCK) #o",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DELETE #o FROM dbo.fnOrders() #o",
                &[(DeleteWithoutWhere, Some("dbo.fnOrders"))],
            ),
            (
                "DELETE #ö FROM dbo.Orders #Ö",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DELETE #o FROM #t CROSS APPLY dbo.Orders #o",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DELETE #o FROM #t CROSS APPLY (SELECT * FROM dbo.Orders) #o",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "DELETE #o FROM OPENQUERY(srv, 'select 1') #o",
                &[(DeleteWithoutWhere, Some("OPENQUERY"))],
            ),
            (
                "WITH #a AS (SELECT * FROM dbo.Orders), #c AS (SELECT * FROM #a) DELETE FROM #c",
                &[(DeleteWithoutWhere, Some("#c"))],
            ),
            (
                "WITH #c AS (SELECT o.Total FROM #t, dbo.Orders o) UPDATE #c SET Total=0",
                &[(UpdateWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "WITH [#c] AS (SELECT * FROM dbo.Orders) DELETE FROM #c",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "WITH \"#c\" AS (SELECT * FROM dbo.Orders) DELETE FROM #c",
                &[(DeleteWithoutWhere, Some("dbo.Orders"))],
            ),
            (
                "WITH #c AS (SELECT * FROM #t) DELETE FROM #c",
                &[(DeleteWithoutWhere, Some("#c"))],
            ),
        ];
        for (sql, w) in cases {
            assert_eq!(kinds(sql), want(w), "{sql}");
        }
    }

    #[test]
    fn a_label_before_an_opener_and_an_alias_named_like_one() {
        use GuardKind::*;
        assert_eq!(
            kinds("DELETE FROM dbo.Orders\nlbl:\nRECEIVE * FROM dbo.Q WHERE x=1"),
            want(&[(DeleteWithoutWhere, Some("dbo.Orders"))])
        );
        for w in ["Throw", "Receive"] {
            let sql =
                format!("DELETE #o FROM #t JOIN dbo.Log {w} ON 1=1 JOIN dbo.Orders #o ON 1=1");
            assert_eq!(
                kinds(&sql),
                want(&[(DeleteWithoutWhere, Some("dbo.Orders"))]),
                "{sql}"
            );
        }
    }

    #[test]
    fn dynamic_sql_built_in_pieces_is_reported_and_prose_is_not() {
        for sql in [
            "DECLARE @sql nvarchar(max) = N'UPDATE ' + QUOTENAME(@t) + N' SET col = NULL'; EXEC(@sql)",
            "DECLARE @sql nvarchar(max) = N'UPDATE dbo.Orders' + CHAR(13) + CHAR(10) + N'SET Total = 0'; EXEC(@sql)",
            "DECLARE @sql nvarchar(max) = N'UPDATE dbo.Orders'; SET @sql += N' SET Total = 0'; EXEC(@sql)",
            "DECLARE @sql nvarchar(max) = N'UPDATE dbo.Orders ' + @setClause; EXEC(@sql)",
            "DECLARE @sql nvarchar(max) = CONCAT(N'UPDATE dbo.Orders', N' SET Total = 0'); EXEC(@sql)",
            "DECLARE @sql nvarchar(max) = N'UPDATE o' + @nl + N'SET Total = 0' + @nl + N'FROM dbo.Orders o'; EXEC(@sql)",
        ] {
            let k = kinds(sql);
            assert!(!k.is_empty(), "{sql}");
        }
        for sql in [
            "RAISERROR('Cannot delete order', 16, 1); EXEC(@sql)",
            "PRINT 'Drop old rows'; EXEC(@sql)",
            "PRINT 'Update finished'; EXEC(@sql)",
            "DECLARE @s nvarchar(max) = N'DELETE FROM ' + QUOTENAME(@t) + N' WHERE Id = @id'; EXEC(@s)",
            "DECLARE @s nvarchar(max) = N'DELETE FROM dbo.Orders WHERE Id = @id'; EXEC(@s)",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
    }

    #[test]
    fn merge_in_a_literal_run_through_a_variable_is_unparsed() {
        use GuardKind::*;
        assert_eq!(
            kinds("DECLARE @s nvarchar(max) = N'MERGE dbo.Orders t USING dbo.X s ON 1=1 WHEN MATCHED THEN DELETE;'; EXEC(@s)"),
            want(&[(DeleteWithoutWhere, Some("dbo.Orders"))])
        );
        let sql = "DECLARE @s nvarchar(max) = N'MERGE dbo.Orders t USING ' + @src + N' s ON 1=1 WHEN MATCHED THEN DELETE;'; EXEC(@s)";
        assert_eq!(kinds(sql), want(&[(Unparsed, None)]), "{sql}");
    }

    #[test]
    fn an_alias_named_throw_without_on_is_not_an_opener() {
        use GuardKind::*;
        for sql in [
            "DELETE #o FROM #t CROSS JOIN dbo.Log Throw CROSS JOIN dbo.Orders #o",
            "DELETE #o FROM #t JOIN dbo.Log Throw WITH (NOLOCK) ON 1=1 JOIN dbo.Orders #o ON 1=1",
            "DELETE #o FROM #t JOIN dbo.Log AS Receive INNER JOIN dbo.Orders #o ON 1=1",
        ] {
            assert_eq!(
                kinds(sql),
                want(&[(DeleteWithoutWhere, Some("dbo.Orders"))]),
                "{sql}"
            );
        }
        assert_eq!(
            kinds("DELETE FROM dbo.Orders THROW 50000, 'x', 1 WHERE x = 1"),
            want(&[(DeleteWithoutWhere, Some("dbo.Orders"))])
        );
    }

    #[test]
    fn an_alias_of_a_temporary_table_is_temporary() {
        for sql in [
            "DELETE t FROM #t t JOIN #u u ON t.id = u.id",
            "UPDATE t SET x = 1 FROM #t t JOIN #s s ON t.id = s.id",
            "DELETE t FROM ##g AS t",
            "DELETE t FROM @v t",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
        for sql in [
            "DELETE t FROM dbo.Orders t JOIN #u u ON t.id = u.id",
            "UPDATE t SET x = 1 FROM #u u JOIN dbo.Orders t ON t.id = u.id",
        ] {
            assert_eq!(kinds(sql).len(), 1, "{sql}");
        }
    }

    #[test]
    fn only_the_cte_list_right_before_the_statement_counts() {
        for sql in [
            "WITH #c AS (SELECT 1 AS x) SELECT * FROM #c; DELETE FROM #c",
            "WITH #c AS (SELECT * FROM dbo.Orders) SELECT * FROM #c\nDELETE FROM #c",
            "WITH #c AS (SELECT * FROM dbo.Orders) SELECT 1; WITH #d AS (SELECT 1 AS y) DELETE FROM #c",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
        assert_eq!(
            kinds(
                "WITH #a AS (SELECT 1 AS x), #c (x) AS (SELECT * FROM dbo.Orders) DELETE FROM #c"
            )
            .len(),
            1
        );
    }

    #[test]
    fn generators_of_commands_are_not_messages() {
        for sql in [
            "DECLARE c CURSOR FOR SELECT N'TRUNCATE TABLE ' + QUOTENAME(name) FROM sys.tables; DECLARE @cmd nvarchar(max); FETCH c INTO @cmd; EXEC(@cmd)",
            "INSERT INTO @cmds (sql) SELECT N'DELETE FROM ' + QUOTENAME(name) FROM sys.tables; EXEC(@s)",
            "INSERT INTO @cmds VALUES (N'DROP TABLE ' + QUOTENAME(@t)); EXEC(@s)",
        ] {
            assert!(!kinds(sql).is_empty(), "{sql}");
        }
        assert_eq!(kinds("THROW 50000, 'drop table x', 1; EXEC(@s)"), vec![]);
    }

    #[test]
    fn merge_needs_using_or_into_to_be_reported() {
        use GuardKind::*;
        assert_eq!(kinds("SET @msg = N'Merge complete'; EXEC(@s)"), vec![]);
        assert_eq!(
            kinds("DECLARE @s nvarchar(max) = N'MERGE INTO dbo.Orders t USING dbo.X s ON 1=1 WHEN MATCHED THEN DELETE;'; EXEC(@s)"),
            want(&[(DeleteWithoutWhere, Some("dbo.Orders"))])
        );
    }

    #[test]
    fn a_where_elsewhere_does_not_make_built_up_sql_safe() {
        for sql in [
            "SET @sql = N'IF EXISTS (SELECT 1 FROM sys.tables WHERE name = ''' + @t + N''') DROP TABLE ' + QUOTENAME(@t); EXEC(@sql)",
            "SET @sql = N'IF EXISTS (SELECT 1 FROM sys.tables WHERE name = @n) DELETE FROM ' + QUOTENAME(@t); EXEC sp_executesql @sql, N'@n sysname', @n = @t",
            "SET @sql = N'DELETE FROM ' + QUOTENAME(@t) + N' WHERE id < 2; DELETE FROM ' + QUOTENAME(@t2); EXEC(@sql)",
            "SET @sql = N'UPDATE x SET v = 0 FROM ' + QUOTENAME(@t) + N' x JOIN (SELECT 1 AS id WHERE 1=1) s ON 1=1'; EXEC(@sql)",
            "SET @sql = N'/* rows where obsolete */ DROP TABLE IF EXISTS ' + QUOTENAME(@t); EXEC(@sql)",
            "SET @sql = N'DELETE FROM ' + @t + N' -- where'; EXEC(@sql)",
            "SET @sql = N'UPDATE ' + QUOTENAME(@t) + N' SET v = (SELECT 1 WHERE 1=1)'; EXEC(@sql)",
        ] {
            assert!(!kinds(sql).is_empty(), "{sql}");
        }
        for sql in [
            "SET @s = N'DELETE FROM ' + QUOTENAME(@t) + N' WHERE Id = @id'; EXEC(@s)",
            "SET @s = N'UPDATE ' + QUOTENAME(@t) + N' SET v = 0 WHERE Id = @id'; EXEC(@s)",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
    }

    #[test]
    fn prose_with_update_is_not_a_statement() {
        for sql in [
            "SET @msg = N'Update finished'; EXEC(@s)",
            "SELECT @msg = N'Update totals'; EXEC(@s)",
            "DECLARE @msg nvarchar(50) = N'Update totals step'; EXEC(@s)",
            "SELECT 1 FROM t WHERE op = 'UPDATE'; EXEC(@s)",
            "EXEC dbo.LogStep @step = N'Update totals'; EXEC(@s)",
        ] {
            assert_eq!(kinds(sql), vec![], "{sql}");
        }
        for sql in [
            "DECLARE @s nvarchar(max) = CONCAT(N'UPDATE ', QUOTENAME(@t), N' SET v = 0'); EXEC(@s)",
            "DECLARE @s nvarchar(max) = CONCAT(N'UPDATE dbo.Orders', N' SET Total = 0'); EXEC(@s)",
        ] {
            assert!(!kinds(sql).is_empty(), "{sql}");
        }
    }

    #[test]
    fn every_dangerous_word_of_a_call_literal_counts() {
        use GuardKind::*;
        for sql in [
            "EXEC sp_MSforeachtable N'UPDATE STATISTICS ?; TRUNCATE TABLE ?'",
            "sp_MSforeachtable N'UPDATE STATISTICS ?; DELETE FROM ?'",
            "sp_MSforeachtable @command1 = N'UPDATE STATISTICS ? WITH FULLSCAN; DROP TABLE ?'",
        ] {
            assert_eq!(kinds(sql), want(&[(Unparsed, None)]), "{sql}");
        }
        assert_eq!(kinds("EXEC dbo.LogStep @step = N'Update totals'"), vec![]);
    }

    #[test]
    fn merge_built_in_parts_is_reported() {
        use GuardKind::*;
        for sql in [
            "SET @sql = N'MERGE ' + QUOTENAME(@t) + N' AS tgt '; SET @sql += N'USING dbo.X s ON 1=1 WHEN NOT MATCHED BY SOURCE THEN DELETE;'; EXEC(@sql)",
            "SET @sql = N'MERGE dbo.Orders AS tgt'; SET @sql += N' USING dbo.X s ON 1=1 WHEN MATCHED THEN DELETE;'; EXEC(@sql)",
            "SET @sql = N'MERGE ' + QUOTENAME(@t) + N' AS tgt'; EXEC(@sql)",
        ] {
            assert!(
                kinds(sql).contains(&(Unparsed, None)),
                "{sql}"
            );
        }
        assert_eq!(kinds("SET @msg = N'Merge complete'; EXEC(@s)"), vec![]);
    }

    #[test]
    fn a_conditional_where_does_not_bound_built_up_sql() {
        for sql in [
            "SET @s = CONCAT(N'DELETE FROM ', QUOTENAME(@t), CASE WHEN @all = 1 THEN N'' ELSE N' WHERE Flag = 1' END); EXEC(@s)",
            "SET @s = CONCAT(N'UPDATE ', QUOTENAME(@t), N' SET Flag = 0', CASE WHEN @id IS NULL THEN N'' ELSE N' WHERE Id = ' + CAST(@id AS nvarchar(10)) END); EXEC(@s)",
            "SET @s = CONCAT(N'DELETE FROM ', QUOTENAME(@t), CASE @mode WHEN 1 THEN N' WHERE a = 1' WHEN 2 THEN N' WHERE b = 2' END); EXEC(@s)",
            "SET @s = N'DELETE FROM ' + QUOTENAME(@t) + CASE WHEN @all = 1 THEN N'' ELSE N' WHERE Flag = 1' END; EXEC(@s)",
            "SET @s = N'DELETE FROM ' + QUOTENAME(@t) + ISNULL(@w, N' WHERE 1 = 1'); EXEC(@s)",
            "SET @s = CONCAT(N'DELETE FROM dbo.', (@t)); EXEC(@s)",
            "SET @s = CONCAT(N'DELETE FROM ', IIF(@a = 1, N'', N' WHERE a = 1')); EXEC(@s)",
        ] {
            assert!(!kinds(sql).is_empty(), "{sql}");
        }
        assert_eq!(
            kinds(
                "SET @s = CONCAT(N'DELETE FROM ', QUOTENAME(@t), N' WHERE Id = ', @id); EXEC(@s)"
            ),
            vec![]
        );
    }
}
