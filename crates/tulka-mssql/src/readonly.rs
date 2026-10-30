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

/// Longest entry of `FORBIDDEN`; longer words cannot match.
const MAX_FORBIDDEN_LEN: usize = 14;

/// A dotted name with this many dots or more (`srv.db.dbo.t`) goes through a
/// linked server.
const LINKED_SERVER_DOTS: u32 = 3;

/// Longest offending word kept in a violation.
const MAX_WORD_CHARS: usize = 40;

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
        // An exponent without digits, or a dot right before a letter.
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

        match t.kind {
            Kind::Punct if is_punct(text, t, "(") => depth += 1,
            Kind::Punct if is_punct(text, t, ")") => {
                if depth == 0 {
                    return Some(found(ReadOnlyReason::Unparsed, word_of(text, t), line));
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
            "SELECT * FROM sys.dm_exec_sessions OPTION (MAXDOP 1)",
            ";WITH c AS (SELECT 1 x) SELECT * FROM c",
            "SELECT 1;;\n;SELECT 2",
        ] {
            check(sql, None);
        }
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
