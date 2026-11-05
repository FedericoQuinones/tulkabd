//! A strict read-only check for T-SQL: only `SELECT` and `WITH ... SELECT`
//! get through, so an AI assistant can query without being able to change
//! anything.
//!
//! The check is lexical and fails closed: it refuses what it cannot tell
//! apart. It is a gate against mistakes and prompt injection, not a
//! substitute for a database login with read-only permissions. Known limits:
//!
//! - A user function or CLR routine called from a query can have external
//!   effects; a name is not enough to tell.
//! - Server files, secrets and other sessions' SQL are reachable through a
//!   few functions and views (see `DENIED_NAMES`); they are refused by name,
//!   with or without the `sys.` prefix, wherever the name appears as a word
//!   or a bracketed name, so a column or alias with that name is refused too.
//!   Other functions and views that read files or secrets may exist; a
//!   login without those permissions is the real limit.
//! - The query hints `MAXDOP`, `MIN_GRANT_PERCENT`, `MAX_GRANT_PERCENT` and
//!   `QUERYTRACEON` are refused inside an `OPTION (...)` clause (a column
//!   named like them is fine elsewhere). `USE HINT` is refused because `USE`
//!   is a forbidden word.
//! - A three-part name (`db.schema.object`) names a database; see
//!   [`referenced_databases`]. A column written with its schema and table
//!   (`dbo.t.col`) looks the same and is taken as database `dbo`.
//! - A synonym or view may point at a linked server; only four-part names
//!   written in the text are caught.
//! - Unbracketed columns named like non-reserved forbidden words (`Send`,
//!   `Get`, `Move`, `Open`, `Close`, `Save`, `Load`) are refused; bracket
//!   them.
//! - The `MERGE` join hint (`INNER MERGE JOIN`) is read-only but is refused
//!   too, because `MERGE` is a forbidden word.
//! - A dotted name with three or more dots is taken as a linked-server name,
//!   so `db.dbo.t.col` and `dbo.t.xmlcol.value(...)` are refused (an accepted
//!   false positive).
//! - Anything the check cannot read with certainty is refused as unparsed:
//!   control characters, a bare `\r`, symbols outside T-SQL's operator set,
//!   Unicode spaces and non-ASCII letters outside bracketed names.
//! - A bracketed or quoted name may only hold printable ASCII, Latin-1
//!   letters and Latin Extended-A letters (without `×`, `÷`, `ſ`), because a
//!   collation can treat ligatures, superscripts, zero-width characters,
//!   circled letters and the like as equal to ASCII ones. Names with other
//!   characters (Cyrillic, Greek, CJK, ...) cannot be used through the AI
//!   tools. Accents and letter case are folded when comparing names with the
//!   denied lists.
//! - A word or literal written right after a number with no space is
//!   refused, because SQL Server splits `1eDELETE` into `1e` and `DELETE`.
//!   This also refuses valid queries such as `1.e5`, `1AND`, `1DESC`,
//!   `1[x]`, `1'x'` and money literals like `$1.50`; add a space or use a
//!   plain number.

use tulka_core::{ReadOnlyReason, ReadOnlyViolation};

use crate::batches::split_batches;
use crate::lexer::{self, Kind, Token};

/// Words that may not appear anywhere in a query, in upper case and sorted.
///
/// - Writes: `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `TRUNCATE`, `INTO`
///   (`SELECT ... INTO` creates a table), `BULK`, the text-column writers
///   `READTEXT`/`WRITETEXT`/`UPDATETEXT`.
/// - Schema and permissions: `ADD` (signatures), `ALTER`, `CREATE`, `DROP`,
///   `GRANT`, `REVOKE`, `DENY`, `TRIGGER` (enable/disable), `BACKUP`,
///   `RESTORE`.
/// - Running code: `EXEC`, `EXECUTE`, `OPENQUERY`, `OPENROWSET`,
///   `OPENDATASOURCE` (they send text to another server).
/// - State and flow: `SET`, `SETUSER`, `USE`, `DECLARE`, `BEGIN`, `COMMIT`,
///   `ROLLBACK`, `SAVE`, `IF`, `WHILE`, `GOTO`, `RETURN`, `BREAK`,
///   `CONTINUE`, `WAITFOR` (holds the connection), `PRINT`, `RAISERROR`,
///   `THROW`, `OPEN`, `CLOSE`, `DEALLOCATE` (cursors).
/// - Server administration: `DBCC`, `KILL`, `SHUTDOWN`, `RECONFIGURE`,
///   `CHECKPOINT`.
/// - Service Broker: `SEND`, `RECEIVE`, `GET`, `MOVE`, `CONVERSATION`;
///   `RECEIVE` removes messages from a queue.
/// - Lock hints: `UPDLOCK`, `XLOCK`, `TABLOCKX` take write locks;
///   `HOLDLOCK`, `SERIALIZABLE`, `REPEATABLEREAD`, `TABLOCK`, `PAGLOCK` hold
///   shared locks to the end of the transaction. They have no place in a
///   query an assistant sends.
/// - `LOAD` and `DUMP` are legacy backup syntax; `ENABLE`/`DISABLE` switch
///   triggers and the like; `REVERT` changes the execution context; `TRAN`,
///   `TRANSACTION` and `CURSOR` open transactions and cursors.
///
/// `NEXT` and `FETCH` are handled apart: `NEXT VALUE FOR` advances a
/// sequence, and `FETCH` is only valid in `OFFSET n ROWS FETCH NEXT|FIRST`.
/// `END` and `ELSE` are allowed because `CASE` uses them.
const FORBIDDEN: &[&str] = &[
    "ADD",
    "ALTER",
    "BACKUP",
    "BEGIN",
    "BREAK",
    "BULK",
    "CHECKPOINT",
    "CLOSE",
    "COMMIT",
    "CONTINUE",
    "CONVERSATION",
    "CREATE",
    "CURSOR",
    "DBCC",
    "DEALLOCATE",
    "DECLARE",
    "DELETE",
    "DENY",
    "DISABLE",
    "DROP",
    "DUMP",
    "ENABLE",
    "EXEC",
    "EXECUTE",
    "GET",
    "GOTO",
    "GRANT",
    "HOLDLOCK",
    "IF",
    "INSERT",
    "INTO",
    "KILL",
    "LOAD",
    "MERGE",
    "MOVE",
    "OPEN",
    "OPENDATASOURCE",
    "OPENQUERY",
    "OPENROWSET",
    "PAGLOCK",
    "PRINT",
    "RAISERROR",
    "READTEXT",
    "RECEIVE",
    "RECONFIGURE",
    "REPEATABLEREAD",
    "RESTORE",
    "RETURN",
    "REVERT",
    "REVOKE",
    "ROLLBACK",
    "SAVE",
    "SEND",
    "SERIALIZABLE",
    "SET",
    "SETUSER",
    "SHUTDOWN",
    "TABLOCK",
    "TABLOCKX",
    "THROW",
    "TRAN",
    "TRANSACTION",
    "TRIGGER",
    "TRUNCATE",
    "UPDATE",
    "UPDATETEXT",
    "UPDLOCK",
    "USE",
    "WAITFOR",
    "WHILE",
    "WRITETEXT",
    "XLOCK",
];

/// Functions and views refused by name, in lower case and sorted: they read
/// files of the server (a UNC path makes a Windows service account
/// authenticate over SMB, and that survives a rollback), password hashes, the
/// log, or the SQL text of other sessions.
const DENIED_NAMES: &[&str] = &[
    "dm_exec_input_buffer",
    "dm_exec_query_plan",
    "dm_exec_query_plan_stats",
    "dm_exec_sql_text",
    "dm_exec_text_query_plan",
    "dm_os_enumerate_filesystem",
    "dm_os_file_exists",
    "fn_dblog",
    "fn_dblog_xtp",
    "fn_dump_dblog",
    "fn_full_dblog",
    "fn_get_audit_file",
    "fn_get_sql",
    "fn_msxe_read_event_stream",
    "fn_trace_gettable",
    "fn_xe_file_target_read_file",
    "loginproperty",
    "query_store_plan",
    "query_store_query_text",
    "sql_logins",
];

/// Query hints that can starve the server, refused inside `OPTION (...)`.
/// In lower case and sorted.
const DENIED_HINTS: &[&str] = &[
    "max_grant_percent",
    "maxdop",
    "min_grant_percent",
    "querytraceon",
];

/// The text of a word or quoted name as written, without its brackets or
/// quotes.
fn unquoted(text: &str, t: &Token) -> String {
    let raw = lexer::src(text, t);
    match (t.kind, raw.chars().next()) {
        (Kind::Quoted, Some('[')) => raw[1..raw.len() - 1].replace("]]", "]"),
        (Kind::Quoted, Some(q)) => {
            raw[1..raw.len() - 1].replace(&format!("{q}{q}"), &q.to_string())
        }
        _ => raw.to_string(),
    }
}

/// A database name for comparing with the selection: lower case and without
/// trailing spaces, but accents kept, because on an accent-sensitive server
/// `Tulkä` and `Tulka` are different databases.
fn database_name_of(text: &str, t: &Token) -> String {
    unquoted(text, t).trim_end().to_lowercase()
}

/// The text of a word or quoted name, lower case, without its brackets or
/// quotes.
fn name_of(text: &str, t: &Token) -> String {
    let inner = unquoted(text, t);
    // SQL Server ignores trailing spaces in an identifier: `[x ]` is `x`.
    // The ideographic space counts as one after folding.
    fold_name(&inner).trim_end().to_string()
}

/// Folds a name the way an accent- and width-insensitive collation compares
/// it: full-width forms to ASCII, accents removed from Latin letters, lower
/// case. Names with characters from other lookalike ranges are refused
/// before this matters (see [`has_unlisted_chars`]).
fn fold_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            c => c,
        })
        .map(base_letter)
        .flat_map(char::to_lowercase)
        .collect()
}

/// A Latin-1 Supplement or Latin Extended-A letter without its accent;
/// other characters unchanged.
fn base_letter(c: char) -> char {
    match c {
        '\u{C0}'..='\u{C5}' | '\u{E0}'..='\u{E5}' | '\u{100}'..='\u{105}' => 'a',
        '\u{C6}' | '\u{E6}' => 'a',
        '\u{C7}' | '\u{E7}' | '\u{106}'..='\u{10D}' => 'c',
        '\u{D0}' | '\u{F0}' | '\u{10E}'..='\u{111}' => 'd',
        '\u{C8}'..='\u{CB}' | '\u{E8}'..='\u{EB}' | '\u{112}'..='\u{11B}' => 'e',
        '\u{11C}'..='\u{123}' => 'g',
        '\u{124}'..='\u{127}' => 'h',
        '\u{CC}'..='\u{CF}' | '\u{EC}'..='\u{EF}' | '\u{128}'..='\u{133}' => 'i',
        '\u{134}'..='\u{135}' => 'j',
        '\u{136}'..='\u{138}' => 'k',
        '\u{139}'..='\u{142}' => 'l',
        '\u{D1}' | '\u{F1}' | '\u{143}'..='\u{14B}' => 'n',
        '\u{D2}'..='\u{D6}' | '\u{D8}' | '\u{F2}'..='\u{F6}' | '\u{F8}' => 'o',
        '\u{14C}'..='\u{153}' => 'o',
        '\u{154}'..='\u{159}' => 'r',
        '\u{DF}' | '\u{15A}'..='\u{161}' | '\u{17F}' => 's',
        '\u{162}'..='\u{167}' => 't',
        '\u{D9}'..='\u{DC}' | '\u{F9}'..='\u{FC}' | '\u{168}'..='\u{173}' => 'u',
        '\u{174}'..='\u{175}' => 'w',
        '\u{DD}' | '\u{FD}' | '\u{FF}' | '\u{176}'..='\u{178}' => 'y',
        '\u{179}'..='\u{17E}' => 'z',
        c => c,
    }
}

/// Whether a quoted name has a character outside the allowed set: printable
/// ASCII, Latin-1 letters and Latin Extended-A letters (without `×`, `÷` and
/// `ſ`). A collation may treat other characters (ligatures, superscripts,
/// zero-width and joiner characters, circled letters, other scripts) as equal
/// to ASCII ones, so a name with any of them cannot be told from a denied
/// one and is refused.
fn has_unlisted_chars(quoted: &str) -> bool {
    let inner = quoted
        .get(1..quoted.len().saturating_sub(1))
        .unwrap_or_default();
    inner.chars().any(|c| {
        !matches!(c as u32,
            0x20..=0x7E | 0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0xFF | 0x100..=0x17E)
    })
}

/// Longest entry of `FORBIDDEN`; longer words cannot match.
const MAX_FORBIDDEN_LEN: usize = 14;

/// A dotted name with this many dots or more (`srv.db.dbo.t`) goes through a
/// linked server.
const LINKED_SERVER_DOTS: u32 = 3;

/// Longest offending word kept in a violation.
const MAX_WORD_CHARS: usize = 40;

/// The databases that three-part names in `sql` point at (`db.schema.obj`,
/// `db..obj`), in lower case, without brackets or quotes, each once. Run
/// after [`violation`] passed: text that does not lex gives nothing here
/// because it was already refused.
pub fn referenced_databases(sql: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for batch in split_batches(sql) {
        let text = &batch.text;
        let Ok(tokens) = lexer::lex(text) else {
            continue;
        };
        let is_name = |t: &Token| matches!(t.kind, Kind::Word | Kind::Quoted);
        let mut i = 0;
        while i < tokens.len() {
            if !is_name(&tokens[i]) {
                i += 1;
                continue;
            }
            // A dotted name: parts separated by dots, a part may be empty.
            let mut parts = vec![database_name_of(text, &tokens[i])];
            i += 1;
            while i < tokens.len() && is_punct(text, &tokens[i], ".") {
                i += 1;
                match tokens.get(i) {
                    Some(t) if is_name(t) => {
                        parts.push(database_name_of(text, t));
                        i += 1;
                    }
                    _ => parts.push(String::new()),
                }
            }
            if parts.len() >= 3 {
                let db = &parts[parts.len() - 3];
                if !db.is_empty() && !out.contains(db) {
                    out.push(db.clone());
                }
            }
        }
    }
    out
}

/// Tells why `sql` is not a plain read-only query, or `None` when it is.
/// Batches separated by `GO` are checked one by one; the first problem wins.
pub fn violation(sql: &str) -> Option<ReadOnlyViolation> {
    let mut any = false;
    for batch in split_batches(sql) {
        let base = batch.start_line - 1;
        let text = &batch.text;
        let control = suspicious_char(text);
        let tokens = match lexer::lex(text) {
            Ok(t) => t,
            Err(offset) => {
                let at = control.map_or(offset, |c| c.min(offset));
                return Some(found(
                    ReadOnlyReason::Unparsed,
                    None,
                    base + line_of(text, at),
                ));
            }
        };
        // The first problem in the text wins, so the walk stops at the
        // first character the server may read differently from the lexer.
        let limit = [control, line_break_outside_strings(text, &tokens)]
            .into_iter()
            .flatten()
            .min();
        if let Some(v) = check_batch(text, &tokens, base, limit, &mut any) {
            return Some(v);
        }
    }
    if any {
        None
    } else {
        Some(found(ReadOnlyReason::Empty, None, 0))
    }
}

fn line_of(text: &str, offset: usize) -> u32 {
    text.as_bytes()[..offset]
        .iter()
        .filter(|&&b| b == b'\n')
        .count() as u32
}

/// Offset of the first character the server may read differently from the
/// lexer: an ASCII control character other than tab, newline and carriage
/// return, or a carriage return in the middle of the text not followed by a
/// newline (the server may end a `--` comment there).
fn suspicious_char(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    b.iter().enumerate().find_map(|(i, &c)| {
        let bad = match c {
            b'\t' | b'\n' => false,
            // A batch ends in `\r` when its script uses CRLF line ends and
            // `GO` follows.
            b'\r' => i + 1 < b.len() && b[i + 1] != b'\n',
            c => c < 0x20 || c == 0x7f,
        };
        bad.then_some(i)
    })
}

/// Offset of the first U+0085, U+2028 or U+2029 outside a string literal.
/// The server may treat them as line ends, which would end a `--` comment
/// where the lexer does not. (Vertical tab and form feed are already refused
/// as control characters.)
fn line_break_outside_strings(text: &str, tokens: &[Token]) -> Option<usize> {
    let b = text.as_bytes();
    (0..b.len()).find(|&i| {
        let hit = match b[i] {
            0xC2 => b.get(i + 1) == Some(&0x85),
            0xE2 => b.get(i + 1) == Some(&0x80) && matches!(b.get(i + 2), Some(0xA8 | 0xA9)),
            _ => false,
        };
        hit && !tokens
            .iter()
            .any(|t| t.kind == Kind::Str && t.start <= i && i < t.end)
    })
}

/// The symbols T-SQL uses outside strings and names. Anything else the lexer
/// emits as punctuation (control characters, Unicode spaces, combining
/// marks) is not understood and is refused.
fn is_known_symbol(text: &str) -> bool {
    matches!(
        text,
        "(" | ")"
            | ","
            | "."
            | ";"
            | "="
            | "<"
            | ">"
            | "!"
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "&"
            | "|"
            | "^"
            | "~"
            | ":"
    )
}

fn found(reason: ReadOnlyReason, word: Option<String>, line: u32) -> ReadOnlyViolation {
    ReadOnlyViolation { reason, word, line }
}

fn word_of(text: &str, t: &Token) -> Option<String> {
    Some(lexer::src(text, t).chars().take(MAX_WORD_CHARS).collect())
}

fn is_punct(text: &str, t: &Token, c: &str) -> bool {
    t.kind == Kind::Punct && lexer::src(text, t) == c
}

fn is_keyword(text: &str, t: &Token, kw: &str) -> bool {
    t.kind == Kind::Word && lexer::src(text, t).eq_ignore_ascii_case(kw)
}

fn is_forbidden(word: &str) -> bool {
    if word.len() > MAX_FORBIDDEN_LEN {
        return false;
    }
    let mut buf = [0u8; MAX_FORBIDDEN_LEN];
    let upper = &mut buf[..word.len()];
    upper.copy_from_slice(word.as_bytes());
    upper.make_ascii_uppercase();
    FORBIDDEN
        .binary_search_by(|f| f.as_bytes().cmp(&*upper))
        .is_ok()
}

/// SQL Server reads numbers more greedily than the lexer: `1eDELETE`,
/// `1.e5DELETE`, `1E-DELETE` and `$1DELETE` are a number followed by a
/// keyword, but the lexer sees one word. Refuse any token that touches the end
/// of a number (or a `.` that touches it) without a gap, and money literals
/// (`$` followed by a digit, `.` or sign).
fn glued_to_number(text: &str, tokens: &[Token], i: usize) -> bool {
    let t = &tokens[i];
    if i > 0 {
        let p = &tokens[i - 1];
        if p.end == t.start {
            if p.kind == Kind::Num && t.kind != Kind::Punct {
                return true;
            }
            // `1.e5DELETE`: a number, a dot, then a word with no gap.
            if t.kind == Kind::Word
                && is_punct(text, p, ".")
                && i > 1
                && tokens[i - 2].kind == Kind::Num
                && tokens[i - 2].end == p.start
            {
                return true;
            }
        }
    }
    if t.kind == Kind::Num {
        let n = lexer::src(text, t);
        let last = n.as_bytes()[n.len() - 1];
        // An exponent without digits. The lexer does not produce such a
        // number today; the check stays in case it ever does.
        if !n.starts_with("0x") && !n.starts_with("0X") && matches!(last, b'e' | b'E' | b'+' | b'-')
        {
            return true;
        }
    }
    if t.kind == Kind::Word {
        let w = lexer::src(text, t).as_bytes();
        if w[0] == b'$' {
            let next = w
                .get(1)
                .copied()
                .or_else(|| text.as_bytes().get(t.end).copied());
            if next.is_some_and(|c| c.is_ascii_digit() || matches!(c, b'.' | b'+' | b'-')) {
                return true;
            }
        }
    }
    false
}

fn check_batch(
    text: &str,
    tokens: &[Token],
    base: u32,
    limit: Option<usize>,
    any: &mut bool,
) -> Option<ReadOnlyViolation> {
    // At the start of a statement (after `;`, or after the parentheses that
    // open one) the head must be a query.
    let mut at_head = true;
    let mut depth = 0u32;
    // Dots in the dotted name being read, and whether the last token was one.
    let mut dots = 0u32;
    let mut after_dot = false;
    // Depth inside the parentheses of an `OPTION (...)` clause.
    let mut option_depth: Option<u32> = None;

    for (i, t) in tokens.iter().enumerate() {
        if limit.is_some_and(|l| t.start >= l) {
            let line = base + line_of(text, limit.unwrap());
            return Some(found(ReadOnlyReason::Unparsed, None, line));
        }
        let line = base + t.line;
        if glued_to_number(text, tokens, i) {
            return Some(found(ReadOnlyReason::Unparsed, None, line));
        }
        let is_dot = is_punct(text, t, ".");

        if is_punct(text, t, ";") {
            if depth > 0 {
                return Some(found(ReadOnlyReason::Unparsed, word_of(text, t), line));
            }
            at_head = true;
            dots = 0;
            after_dot = false;
            continue;
        }
        *any = true;

        if t.kind == Kind::Punct && !is_known_symbol(lexer::src(text, t)) {
            return Some(found(ReadOnlyReason::Unparsed, None, line));
        }

        if at_head {
            if is_punct(text, t, "(") {
                depth += 1;
                continue;
            }
            if !(is_keyword(text, t, "SELECT") || is_keyword(text, t, "WITH")) {
                return Some(found(ReadOnlyReason::NotASelect, word_of(text, t), line));
            }
            at_head = false;
            continue;
        }

        if t.kind == Kind::Quoted && has_unlisted_chars(lexer::src(text, t)) {
            return Some(found(ReadOnlyReason::Unparsed, None, line));
        }
        if matches!(t.kind, Kind::Word | Kind::Quoted) {
            let name = name_of(text, t);
            if DENIED_NAMES.binary_search(&name.as_str()).is_ok()
                || (option_depth.is_some() && DENIED_HINTS.binary_search(&name.as_str()).is_ok())
            {
                return Some(found(ReadOnlyReason::Forbidden, word_of(text, t), line));
            }
        }

        match t.kind {
            Kind::Punct if is_punct(text, t, "(") => {
                depth += 1;
                if option_depth.is_none() && i > 0 && is_keyword(text, &tokens[i - 1], "OPTION") {
                    option_depth = Some(depth);
                }
            }
            Kind::Punct if is_punct(text, t, ")") => {
                if depth == 0 {
                    return Some(found(ReadOnlyReason::Unparsed, word_of(text, t), line));
                }
                if option_depth == Some(depth) {
                    option_depth = None;
                }
                depth -= 1;
            }
            Kind::Word => {
                let w = lexer::src(text, t);
                if !w.is_ascii() {
                    return Some(found(ReadOnlyReason::Unparsed, word_of(text, t), line));
                }
                if is_forbidden(w) {
                    return Some(found(ReadOnlyReason::Forbidden, word_of(text, t), line));
                }
                if w.eq_ignore_ascii_case("NEXT")
                    && tokens
                        .get(i + 1)
                        .is_some_and(|n| is_keyword(text, n, "VALUE"))
                {
                    return Some(found(ReadOnlyReason::Forbidden, word_of(text, t), line));
                }
                // `FETCH` is valid only as `ROWS FETCH NEXT|FIRST`.
                if w.eq_ignore_ascii_case("FETCH") {
                    let after_rows = i > 0
                        && (is_keyword(text, &tokens[i - 1], "ROW")
                            || is_keyword(text, &tokens[i - 1], "ROWS"));
                    let then_next = tokens.get(i + 1).is_some_and(|n| {
                        is_keyword(text, n, "NEXT") || is_keyword(text, n, "FIRST")
                    });
                    // The row count is a number, a variable or an expression.
                    let then_count = tokens.get(i + 2).is_some_and(|c| {
                        matches!(c.kind, Kind::Num | Kind::Var) || is_punct(text, c, "(")
                    });
                    if !(after_rows && then_next && then_count) {
                        return Some(found(ReadOnlyReason::Forbidden, word_of(text, t), line));
                    }
                }
            }
            _ => {}
        }

        // Count the dots of a dotted name; empty parts (`db..t`) count too.
        if is_dot {
            dots += 1;
            after_dot = true;
            if dots >= LINKED_SERVER_DOTS {
                return Some(found(ReadOnlyReason::LinkedServer, None, line));
            }
        } else if matches!(t.kind, Kind::Word | Kind::Quoted) {
            if !after_dot {
                dots = 0;
            }
            after_dot = false;
        } else {
            dots = 0;
            after_dot = false;
        }
    }
    if let Some(l) = limit {
        return Some(found(
            ReadOnlyReason::Unparsed,
            None,
            base + line_of(text, l),
        ));
    }
    if depth > 0 {
        let last = tokens.last().map_or(0, |t| t.line);
        return Some(found(ReadOnlyReason::Unparsed, None, base + last));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ReadOnlyReason::*;

    fn why(sql: &str) -> Option<(ReadOnlyReason, Option<String>)> {
        violation(sql).map(|v| (v.reason, v.word.map(|w| w.to_uppercase())))
    }

    fn check(sql: &str, want: Option<(ReadOnlyReason, Option<&str>)>) {
        let want = want.map(|(r, w)| (r, w.map(String::from)));
        assert_eq!(why(sql), want, "{sql}");
    }

    #[test]
    fn forbidden_list_is_sorted_and_upper_case() {
        assert!(FORBIDDEN.windows(2).all(|w| w[0] < w[1]));
        assert!(FORBIDDEN.iter().all(|w| *w == w.to_uppercase()));
        assert_eq!(
            FORBIDDEN.iter().map(|w| w.len()).max(),
            Some(MAX_FORBIDDEN_LEN)
        );
    }

    #[test]
    fn accepts_reading_queries() {
        for sql in [
            "SELECT 1",
            "SELECT 1e5, 1.5E-3, 0x1F, 1.5, .5, 1 + 2, (1), $partition.pf(1), $action",
            "SELECT TOP 10 a FROM t WHERE a IN (1,2) AND b=1.",
            "select * from dbo.Pedidos where id = 4",
            "WITH c AS (SELECT id FROM t) SELECT * FROM c",
            "WITH a AS (SELECT 1 x), b AS (SELECT x FROM a) SELECT * FROM b;",
            "SELECT * FROM t ORDER BY id OFFSET 10 ROWS FETCH NEXT 10 ROWS ONLY",
            "SELECT CASE WHEN a > 1 THEN 'x' ELSE 'y' END FROM t",
            "SELECT IIF(a > 1, 1, 0) FROM t WITH (NOLOCK)",
            "SELECT [update], [delete] FROM [insert]",
            "SELECT 'DELETE FROM t; DROP TABLE t' AS s -- EXEC xp_cmdshell\n/* INSERT INTO t */",
            "SELECT N'TRUNCATE TABLE t'",
            "SELECT * FROM OtraBase.dbo.Clientes",
            "SELECT * FROM OtraBase..Clientes",
            "SELECT dbo.Orders.id FROM dbo.Orders",
            "SELECT * FROM #tmp",
            "SELECT a FROM t FOR JSON PATH",
            "SELECT * FROM OPENJSON(N'[1,2]')",
            "SELECT ROW_NUMBER() OVER (PARTITION BY a ORDER BY b) FROM t",
            "(SELECT 1) UNION ALL (SELECT 2)",
            "SELECT 1; SELECT 2;;",
            "SELECT 1\nGO\nSELECT 2",
            "SELECT OBJECT_DEFINITION(OBJECT_ID('dbo.v'))",
            "WITH XMLNAMESPACES ('urn:x' AS x) SELECT 1",
            "SELECT * FROM sys.dm_exec_sessions OPTION (RECOMPILE, HASH JOIN)",
            "SELECT maxdop, [min_grant_percent] FROM t OPTION (RECOMPILE)",
            "SELECT 'sys.sql_logins', 'fn_dblog' AS s",
            "SELECT name FROM sys.sql_modules",
            ";WITH c AS (SELECT 1 x) SELECT * FROM c",
            "SELECT 1;;\n;SELECT 2",
        ] {
            check(sql, None);
        }
    }

    #[test]
    fn finds_the_databases_of_three_part_names() {
        let dbs = |sql: &str| referenced_databases(sql);
        assert_eq!(dbs("SELECT 1"), Vec::<String>::new());
        assert_eq!(
            dbs("SELECT * FROM dbo.t JOIN sys.tables x ON 1=1"),
            Vec::<String>::new()
        );
        assert_eq!(dbs("SELECT * FROM Other.sys.tables"), ["other"]);
        assert_eq!(dbs("SELECT * FROM [Other DB].[dbo].[t]"), ["other db"]);
        assert_eq!(dbs("SELECT * FROM \"Other\".dbo.t"), ["other"]);
        assert_eq!(dbs("SELECT * FROM [a]]b].dbo.t"), ["a]b"]);
        assert_eq!(dbs("SELECT * FROM [Other ].dbo.t"), ["other"]);
        assert_eq!(dbs("SELECT * FROM \"Other  \"..t"), ["other"]);
        assert_eq!(dbs("SELECT * FROM Other..t"), ["other"]);
        assert_eq!(
            dbs("SELECT * FROM a . dbo . t, A.dbo.u, b.dbo.v"),
            ["a", "b"]
        );
        assert_eq!(
            dbs("SELECT 1; SELECT * FROM x.dbo.t\nGO\nSELECT * FROM y.dbo.t"),
            ["x", "y"]
        );
        // A schema-qualified column looks the same.
        assert_eq!(dbs("SELECT dbo.t.id FROM dbo.t"), ["dbo"]);
        // Text in strings and comments is not a name.
        assert_eq!(dbs("SELECT 'a.b.c' -- d.e.f"), Vec::<String>::new());
    }

    #[test]
    fn denied_names_have_no_letter_pair_a_collation_expands_to() {
        // On the server, Æ Þ ß Ĳ Œ compare equal to ae th ss ij oe.
        for name in DENIED_NAMES.iter().chain(DENIED_HINTS) {
            for pair in ["ae", "th", "ss", "ij", "oe"] {
                assert!(!name.contains(pair), "{name} contains {pair}");
            }
        }
    }

    #[test]
    fn denied_lists_are_sorted_and_lower_case() {
        for list in [DENIED_NAMES, DENIED_HINTS] {
            assert!(list.windows(2).all(|w| w[0] < w[1]));
            assert!(list.iter().all(|w| *w == w.to_lowercase()));
        }
    }

    #[test]
    fn refuses_file_secret_and_sql_text_sources() {
        for sql in [
            r"SELECT * FROM sys.fn_xe_file_target_read_file('\\h\s\x.xel', NULL, NULL, NULL)",
            "SELECT * FROM fn_xe_file_target_read_file('a', NULL, NULL, NULL)",
            "SELECT * FROM [sys].[fn_get_audit_file]('a', DEFAULT, DEFAULT)",
            "SELECT * FROM sys.\"FN_TRACE_GETTABLE\"('a', 1)",
            "SELECT * FROM fn_dump_dblog(NULL)",
            "SELECT * FROM sys.fn_dblog(NULL, NULL)",
            "SELECT * FROM sys.dm_os_enumerate_filesystem('/', '*')",
            "SELECT * FROM sys.dm_os_file_exists('/etc/passwd')",
            "SELECT * FROM sys.fn_get_sql(NULL)",
            "SELECT name, password_hash FROM sys.sql_logins",
            "SELECT name, password_hash FROM master.sys.[SQL_Logins]",
            "SELECT t.text FROM sys.dm_exec_requests r CROSS APPLY sys.dm_exec_sql_text(r.sql_handle) t",
            "SELECT * FROM sys.dm_exec_input_buffer(52, NULL)",
            "SELECT * FROM sys . dm_exec_sql_text(NULL)",
            "SELECT * FROM sys.[sql_logins ]",
            "SELECT * FROM sys.[sql_lógins]",
            "SELECT * FROM sys.[ŠQL_loġins]",
            "SELECT * FROM [sys].[fn_get_audit_file  ]('a', DEFAULT, DEFAULT)",
            "SELECT * FROM sys.\"sql_logins \"",
            "SELECT LOGINPROPERTY('sa', 'PasswordHash')",
            "SELECT * FROM sys.dm_exec_query_plan(0x)",
            "SELECT * FROM sys.dm_exec_text_query_plan(0x, 0, -1)",
            "SELECT * FROM sys.dm_exec_query_plan_stats(0x)",
            "SELECT * FROM sys.query_store_query_text",
            "SELECT * FROM sys.query_store_plan",
            "SELECT * FROM fn_full_dblog(NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL)",
            "SELECT * FROM sys.fn_MSxe_read_event_stream('s', 0, 0)",
        ] {
            let v = violation(sql).unwrap_or_else(|| panic!("accepted: {sql}"));
            assert_eq!(v.reason, Forbidden, "{sql}");
        }
    }

    #[test]
    fn only_listed_characters_are_allowed_in_quoted_names() {
        for sql in [
            "SELECT * FROM sys.[fn_get_audit_\u{FB01}le]('a', DEFAULT, DEFAULT)",
            "SELECT * FROM sys.[fn_xe_\u{FB01}le_target_read_file]('a', NULL, NULL, NULL)",
            "SELECT * FROM sys.[f\u{207F}_get_audit_file]('a', DEFAULT, DEFAULT)",
            "SELECT * FROM sys.[sql_lo\u{200D}gins]",
            "SELECT * FROM sys.[sql_lo\u{FEFF}gins]",
            "SELECT * FROM sys.[sql_lo\u{2060}gins]",
            "SELECT * FROM sys.[sql_l\u{BA}gins]",
            "SELECT * FROM sys.[sql_l\u{AA}gins]",
            "SELECT * FROM sys.[\u{24E2}ql_logins]",
            "SELECT * FROM sys.[\u{FF33}QL_logins]",
            "SELECT * FROM [t\u{0301}]",
            "SELECT * FROM [\u{1D41A}b]",
            "SELECT * FROM [a\u{2102}]",
            "SELECT * FROM [a\u{FE0F}]",
            "SELECT * FROM [a\u{17F}]",
            "SELECT * FROM [a\u{D7}]",
            "SELECT * FROM [a\u{F7}]",
            "SELECT * FROM [\u{416}]",
            "SELECT \"x\u{4E2D}\" FROM t",
        ] {
            let v = violation(sql).unwrap_or_else(|| panic!("accepted: {sql}"));
            assert_eq!(v.reason, Unparsed, "{sql}");
        }
        for sql in [
            "SELECT * FROM [Clientes_A\u{F1}o]",
            "SELECT [\u{D1}and\u{FA}], [M\u{FC}ller], [\u{10A}] FROM [Pedidos_\u{DC}nicos]",
            "SELECT * FROM [Mi tabla]",
        ] {
            assert!(violation(sql).is_none(), "{sql}");
        }
        // Accents are kept: another database may differ only by them.
        assert_eq!(
            referenced_databases("SELECT * FROM [\u{D3}tro].dbo.t"),
            ["\u{F3}tro"]
        );
        assert_eq!(
            referenced_databases("SELECT * FROM [Tulk\u{E4}_mcp ].dbo.t"),
            ["tulk\u{E4}_mcp"]
        );
    }

    #[test]
    fn refuses_starving_hints_only_inside_option() {
        for sql in [
            "SELECT * FROM t OPTION (MAXDOP 1)",
            "SELECT * FROM t option(maxdop 0)",
            "SELECT * FROM t OPTION (RECOMPILE, MIN_GRANT_PERCENT = 100)",
            "SELECT * FROM t OPTION (MAX_GRANT_PERCENT = 100)",
            "SELECT * FROM t OPTION (QUERYTRACEON 9481)",
            "SELECT * FROM t OPTION (HASH JOIN, [MAXDOP] 1)",
            "SELECT * FROM (SELECT 1 a) x OPTION ((MAXDOP 1))",
        ] {
            let v = violation(sql).unwrap_or_else(|| panic!("accepted: {sql}"));
            assert_eq!(v.reason, Forbidden, "{sql}");
        }
        check(
            "SELECT * FROM t OPTION (USE HINT ('X'))",
            Some((Forbidden, Some("USE"))),
        );
        check("SELECT maxdop FROM t OPTION (RECOMPILE) ", None);
        check(
            "SELECT * FROM t OPTION (RECOMPILE); SELECT maxdop FROM t",
            None,
        );
    }

    #[test]
    fn rejects_everything_else() {
        let cases: &[(&str, (ReadOnlyReason, Option<&str>))] = &[
            ("", (Empty, None)),
            ("  -- only a comment\n", (Empty, None)),
            (";;", (Empty, None)),
            ("INSERT INTO t VALUES (1)", (NotASelect, Some("INSERT"))),
            ("SELECT * INTO #x FROM t", (Forbidden, Some("INTO"))),
            ("SELECT * INTO dbo.copia FROM t", (Forbidden, Some("INTO"))),
            ("SELECT 1; DELETE FROM t", (NotASelect, Some("DELETE"))),
            ("SELECT 1 DELETE FROM t", (Forbidden, Some("DELETE"))),
            (
                "WITH c AS (SELECT * FROM t) DELETE FROM c",
                (Forbidden, Some("DELETE")),
            ),
            (
                "WITH c AS (SELECT * FROM t) UPDATE c SET a = 1",
                (Forbidden, Some("UPDATE")),
            ),
            (
                "WITH c AS (SELECT 1 x) INSERT INTO t SELECT x FROM c",
                (Forbidden, Some("INSERT")),
            ),
            ("EXEC sp_who", (NotASelect, Some("EXEC"))),
            ("sp_who", (NotASelect, Some("SP_WHO"))),
            ("SELECT 1 EXEC('DROP TABLE t')", (Forbidden, Some("EXEC"))),
            (
                "SELECT * FROM OPENQUERY(srv, 'DELETE FROM t')",
                (Forbidden, Some("OPENQUERY")),
            ),
            (
                "SELECT * FROM OPENROWSET(BULK '/etc/passwd', SINGLE_CLOB) x",
                (Forbidden, Some("OPENROWSET")),
            ),
            ("SELECT * FROM srv.db.dbo.t", (LinkedServer, None)),
            ("SELECT * FROM srv.db..t", (LinkedServer, None)),
            ("SELECT * FROM srv . db . dbo . t", (LinkedServer, None)),
            ("SELECT NEXT VALUE FOR dbo.seq", (Forbidden, Some("NEXT"))),
            (
                "SELECT * FROM t WITH (UPDLOCK, HOLDLOCK)",
                (Forbidden, Some("UPDLOCK")),
            ),
            (
                "SELECT * FROM t WITH (TABLOCKX)",
                (Forbidden, Some("TABLOCKX")),
            ),
            ("SET ROWCOUNT 0; SELECT * FROM t", (NotASelect, Some("SET"))),
            ("SELECT 1; COMMIT", (NotASelect, Some("COMMIT"))),
            ("SELECT 1 COMMIT", (Forbidden, Some("COMMIT"))),
            ("BEGIN TRAN SELECT 1", (NotASelect, Some("BEGIN"))),
            (
                "SELECT 1 WAITFOR DELAY '00:01'",
                (Forbidden, Some("WAITFOR")),
            ),
            (
                "DECLARE @x int = 1; SELECT @x",
                (NotASelect, Some("DECLARE")),
            ),
            ("USE master; SELECT 1", (NotASelect, Some("USE"))),
            ("SELECT 1 RECEIVE * FROM q", (Forbidden, Some("RECEIVE"))),
            ("DBCC CHECKDB", (NotASelect, Some("DBCC"))),
            ("TRUNCATE TABLE t", (NotASelect, Some("TRUNCATE"))),
            ("SELECT 1\nGO\nDROP TABLE t", (NotASelect, Some("DROP"))),
            ("(DELETE FROM t)", (NotASelect, Some("DELETE"))),
            ("SELECT 1 IF 1 = 1 SELECT 2", (Forbidden, Some("IF"))),
            ("SELECT 'unclosed", (Unparsed, None)),
            // Keywords glued to numbers: SQL Server splits these differently
            // from the lexer.
            ("SELECT 1eDELETE FROM t", (Unparsed, None)),
            ("SELECT 1eCOMMIT", (Unparsed, None)),
            ("SELECT 1eEXEC('x')", (Unparsed, None)),
            ("SELECT $1DELETE FROM t", (Unparsed, None)),
            ("SELECT $1SELECT 5", (Unparsed, None)),
            ("SELECT $1.50", (Unparsed, None)),
            ("SELECT $-1", (Unparsed, None)),
            ("SELECT 1.e5DELETE FROM t", (Unparsed, None)),
            ("SELECT 1.e5SELECT 6", (Unparsed, None)),
            ("SELECT 1.5eSELECT 7", (Unparsed, None)),
            ("SELECT .5eDELETE FROM t", (Unparsed, None)),
            ("SELECT 1E-DELETE FROM t", (Unparsed, None)),
            ("SELECT 0x1FDELETE FROM t", (Unparsed, None)),
            ("SELECT 1'x'", (Unparsed, None)),
            (
                "SELECT 1eDELETE FROM dbo.T SELECT 1eCOMMIT",
                (Unparsed, None),
            ),
            ("SELECT 1 /* unclosed", (Unparsed, None)),
            ("SELECT [unclosed FROM t", (Unparsed, None)),
            // Beyond the table: parentheses must balance, a `;` cannot sit
            // inside them, and non-word heads are not queries.
            ("SELECT 1) ; DELETE FROM t", (Unparsed, Some(")"))),
            ("SELECT (1; DELETE FROM t)", (Unparsed, Some(";"))),
            ("SELECT (1", (Unparsed, None)),
            ("'SELECT 1'", (NotASelect, Some("'SELECT 1'"))),
            ("[select] 1", (NotASelect, Some("[SELECT]"))),
        ];
        for (sql, want) in cases {
            check(sql, Some(*want));
        }
    }

    #[test]
    fn every_statement_keyword_is_refused_inside_a_query() {
        // The statement keywords of T-SQL (SQL Server 2016+ and Azure SQL)
        // that are not part of a query.
        let words = "INSERT UPDATE DELETE MERGE DROP TRUNCATE CREATE ALTER EXEC EXECUTE \
            DECLARE SET USE GRANT DENY REVOKE BEGIN COMMIT ROLLBACK SAVE TRAN TRANSACTION \
            WAITFOR IF WHILE RETURN GOTO BREAK CONTINUE PRINT RAISERROR THROW CURSOR OPEN \
            CLOSE DEALLOCATE FETCH RECEIVE SEND MOVE GET DBCC KILL CHECKPOINT BACKUP RESTORE \
            SHUTDOWN RECONFIGURE DUMP LOAD BULK WRITETEXT UPDATETEXT READTEXT OPENQUERY \
            OPENROWSET OPENDATASOURCE INTO ENABLE DISABLE SETUSER REVERT UPDLOCK XLOCK \
            TABLOCK TABLOCKX HOLDLOCK PAGLOCK SERIALIZABLE REPEATABLEREAD ADD TRIGGER \
            CONVERSATION";
        for w in words.split_whitespace() {
            for sql in [
                format!("SELECT 1 {w}"),
                format!("SELECT 1 {}", w.to_lowercase()),
                format!("SELECT 1 /* x */ {w} -- y"),
            ] {
                let v = violation(&sql).unwrap_or_else(|| panic!("{sql}"));
                assert_eq!(v.reason, Forbidden, "{sql}");
                assert_eq!(v.word.as_deref().map(str::to_uppercase), Some(w.into()));
            }
        }
    }

    #[test]
    fn fetch_is_only_valid_after_rows() {
        check(
            "SELECT 1 FETCH NEXT FROM c",
            Some((Forbidden, Some("FETCH"))),
        );
        check(
            "SELECT 1 ORDER BY a OFFSET 1 ROW FETCH FIRST 1 ROW ONLY",
            None,
        );
        check(
            "SELECT 1 ORDER BY a OFFSET 1 ROWS FETCH FROM c",
            Some((Forbidden, Some("FETCH"))),
        );
    }

    #[test]
    fn fetch_needs_a_row_count() {
        for sql in [
            "SELECT 1 ROWS FETCH NEXT FROM c",
            "SELECT 1 ROWS FETCH NEXT c",
            "SELECT 1 ROWS FETCH FIRST",
        ] {
            check(sql, Some((Forbidden, Some("FETCH"))));
        }
        check(
            "SELECT 1 ORDER BY a OFFSET 1 ROWS FETCH NEXT @n ROWS ONLY",
            None,
        );
        check(
            "SELECT 1 ORDER BY a OFFSET 1 ROWS FETCH NEXT (1+1) ROWS ONLY",
            None,
        );
    }

    #[test]
    fn crlf_scripts_with_go_are_accepted() {
        check("SELECT 1\r\nGO\r\nSELECT 2", None);
        check("SELECT 1\r\n\r\nGO\r\nSELECT 2\r\n", None);
        check("SELECT 1 -- c\r\nGO\r\nSELECT 2", None);
        check("SELECT 1\rFROM t", Some((Unparsed, None)));
        check(
            "SELECT 1\r\nGO\r\nSELECT 1 -- c\rDELETE",
            Some((Unparsed, None)),
        );
    }

    #[test]
    fn the_first_problem_in_the_text_wins() {
        let v = violation("SELECT 1 DELETE FROM t\n\u{1}").unwrap();
        assert_eq!((v.reason, v.line), (Forbidden, 0));
        let v = violation("SELECT 1\n\u{1} DELETE FROM t").unwrap();
        assert_eq!((v.reason, v.line), (Unparsed, 1));
        let v = violation("SELECT 1 -- a\u{2028}b\nDELETE FROM t").unwrap();
        assert_eq!((v.reason, v.line), (Unparsed, 0));
        let v = violation("SELECT 1\n'unclosed \u{1}").unwrap();
        assert_eq!((v.reason, v.line), (Unparsed, 1));
    }

    #[test]
    fn four_part_column_names_are_refused() {
        check(
            "SELECT db.dbo.t.col FROM db.dbo.t",
            Some((LinkedServer, None)),
        );
        check(
            "SELECT dbo.t.xmlcol.value('x', 'int') FROM dbo.t",
            Some((LinkedServer, None)),
        );
    }

    #[test]
    fn refuses_what_it_cannot_read_with_certainty() {
        for sql in [
            // A control or Unicode space between name parts could hide dots.
            "SELECT * FROM srv.db.\u{1}dbo.t",
            "SELECT * FROM srv.db.\u{1f}.t",
            "SELECT * FROM srv.db.\u{a0}dbo.t",
            "SELECT * FROM srv.db.\u{2003}dbo.t",
            "SELECT * FROM srv.db.\u{301}dbo.t",
            "SELECT 1 \u{0b} DELETE FROM t",
            "SELECT 1 -- c\rDELETE FROM t",
            "SELECT 'a\u{0}b'",
            "SELECT 1 {fn NOW()}",
            "SELECT 1 \\ 2",
            // Non-ASCII letters in an unquoted word.
            "SELECT 1 \u{131}NSERT",
            "SELECT * FROM taé",
        ] {
            let v = violation(sql).unwrap_or_else(|| panic!("{sql:?}"));
            assert_eq!(v.reason, Unparsed, "{sql:?}");
        }
        // Unicode line ends hide in comments and names, not only in code.
        for c in ['\u{85}', '\u{2028}', '\u{2029}', '\u{b}', '\u{c}'] {
            for sql in [
                format!("SELECT 1 -- a{c}DELETE FROM t"),
                format!("SELECT 1 /* a{c}b */"),
                format!("SELECT [a{c}b] FROM t"),
                format!("SELECT 1{c}FROM t"),
            ] {
                let v = violation(&sql).unwrap_or_else(|| panic!("{sql:?}"));
                assert_eq!(v.reason, Unparsed, "{sql:?}");
            }
        }
        for c in ['\u{85}', '\u{2028}', '\u{2029}'] {
            check(&format!("SELECT 'a{c}b', N'{c}'"), None);
        }
        // Bracketed and quoted names, strings and CRLF line ends stay fine.
        for sql in [
            "SELECT [taé], \"ñ\", 'é\u{a0}' FROM [ü]",
            "SELECT 1 -- c\r\nFROM t",
        ] {
            check(sql, None);
        }
    }

    #[test]
    fn caps_the_reported_word() {
        let long = "x".repeat(100);
        let v = violation(&long).unwrap();
        assert_eq!(v.reason, NotASelect);
        assert_eq!(v.word.unwrap().len(), 40);
    }

    #[test]
    fn reports_the_line_of_the_problem() {
        assert_eq!(
            violation("SELECT 1\n\nSELECT * INTO x FROM t")
                .unwrap()
                .line,
            2
        );
        assert_eq!(violation("SELECT 1\nGO\n\nDROP TABLE t").unwrap().line, 3);
        assert_eq!(violation("SELECT 1\n/* a\nb").unwrap().line, 1);
    }
}
