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
//! - `MERGE` is refused as a word, but the `MERGE` join hint
//!   (`INNER MERGE JOIN`) is a read-only hint that is refused with it.

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
/// - Lock hints that take write locks: `HOLDLOCK`, `UPDLOCK`, `XLOCK`,
///   `TABLOCKX`.
/// - `LOAD` restores from a dump in legacy syntax.
///
/// `NEXT` is handled apart (`NEXT VALUE FOR` advances a sequence).
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
    "DBCC",
    "DEALLOCATE",
    "DECLARE",
    "DELETE",
    "DENY",
    "DROP",
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
    "PRINT",
    "RAISERROR",
    "READTEXT",
    "RECEIVE",
    "RECONFIGURE",
    "RESTORE",
    "RETURN",
    "REVOKE",
    "ROLLBACK",
    "SAVE",
    "SEND",
    "SET",
    "SETUSER",
    "SHUTDOWN",
    "TABLOCKX",
    "THROW",
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
        let tokens = match lexer::lex(&batch.text) {
            Ok(t) => t,
            Err(offset) => {
                let line = batch.text.as_bytes()[..offset]
                    .iter()
                    .filter(|&&b| b == b'\n')
                    .count() as u32;
                return Some(found(ReadOnlyReason::Unparsed, None, base + line));
            }
        };
        if let Some(v) = check_batch(&batch.text, &tokens, base, &mut any) {
            return Some(v);
        }
    }
    if any {
        None
    } else {
        Some(found(ReadOnlyReason::Empty, None, 0))
    }
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

fn check_batch(
    text: &str,
    tokens: &[Token],
    base: u32,
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
        let line = base + t.line;
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
