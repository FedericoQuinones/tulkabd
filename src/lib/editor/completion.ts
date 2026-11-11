// SQL autocompletion from the cached metadata of the tab's database: keywords,
// schemas, tables and views, and columns after `alias.` or `table.`.
import type { Completion, CompletionContext, CompletionResult, CompletionSource } from "@codemirror/autocomplete";
import * as ipc from "../ipc";
import type { CompletionMetadata, DbObject } from "../ipc";
import { quoteIdent } from "../explorer/tree";

/** T-SQL reserved keywords: an identifier spelled like one needs brackets. */
const RESERVED = new Set(
  `ADD ALL ALTER AND ANY AS ASC AUTHORIZATION BACKUP BEGIN BETWEEN BREAK BROWSE BULK BY CASCADE CASE CHECK
  CHECKPOINT CLOSE CLUSTERED COALESCE COLLATE COLUMN COMMIT COMPUTE CONSTRAINT CONTAINS CONTAINSTABLE CONTINUE
  CONVERT CREATE CROSS CURRENT CURRENT_DATE CURRENT_TIME CURRENT_TIMESTAMP CURRENT_USER CURSOR DATABASE DBCC
  DEALLOCATE DECLARE DEFAULT DELETE DENY DESC DISK DISTINCT DISTRIBUTED DOUBLE DROP DUMP ELSE END ERRLVL ESCAPE
  EXCEPT EXEC EXECUTE EXISTS EXIT EXTERNAL FETCH FILE FILLFACTOR FOR FOREIGN FREETEXT FREETEXTTABLE FROM FULL
  FUNCTION GOTO GRANT GROUP HAVING HOLDLOCK IDENTITY IDENTITY_INSERT IDENTITYCOL IF IN INDEX INNER INSERT
  INTERSECT INTO IS JOIN KEY KILL LEFT LIKE LINENO LOAD MERGE NATIONAL NOCHECK NONCLUSTERED NOT NULL NULLIF OF
  OFF OFFSETS ON OPEN OPENDATASOURCE OPENQUERY OPENROWSET OPENXML OPTION OR ORDER OUTER OVER PERCENT PIVOT PLAN
  PRECISION PRIMARY PRINT PROC PROCEDURE PUBLIC RAISERROR READ READTEXT RECONFIGURE REFERENCES REPLICATION
  RESTORE RESTRICT RETURN REVERT REVOKE RIGHT ROLLBACK ROWCOUNT ROWGUIDCOL RULE SAVE SCHEMA SECURITYAUDIT SELECT
  SEMANTICKEYPHRASETABLE SEMANTICSIMILARITYDETAILSTABLE SEMANTICSIMILARITYTABLE SESSION_USER SET SETUSER SHUTDOWN
  SOME STATISTICS SYSTEM_USER TABLE TABLESAMPLE TEXTSIZE THEN TO TOP TRAN TRANSACTION TRIGGER TRUNCATE
  TRY_CONVERT TSEQUAL UNION UNIQUE UNPIVOT UPDATE UPDATETEXT USE USER VALUES VARYING VIEW WAITFOR WHEN WHERE
  WHILE WITH WITHIN WRITETEXT`.split(/\s+/),
);

/** Offered as completions. */
const KEYWORDS = `SELECT TOP DISTINCT FROM WHERE AND OR NOT IN EXISTS BETWEEN LIKE IS NULL AS ON JOIN INNER LEFT RIGHT
  FULL OUTER CROSS APPLY GROUP BY HAVING ORDER ASC DESC UNION ALL EXCEPT INTERSECT INSERT INTO VALUES UPDATE SET
  DELETE MERGE TRUNCATE TABLE VIEW PROCEDURE FUNCTION INDEX CREATE ALTER DROP EXEC DECLARE BEGIN END IF ELSE WHILE
  RETURN CASE WHEN THEN WITH NOLOCK OFFSET FETCH NEXT ROWS ONLY OVER PARTITION COUNT SUM AVG MIN MAX CAST CONVERT
  ISNULL COALESCE GETDATE TRANSACTION COMMIT ROLLBACK PRINT`.split(/\s+/);

const PLAIN = /^[\p{L}_][\p{L}\p{N}_@#$]*$/u;

/** A name as SQL Server needs it written: bracketed when it is not a regular identifier or is reserved. */
export function sqlName(name: string): string {
  return PLAIN.test(name) && !RESERVED.has(name.toUpperCase()) ? name : quoteIdent(name);
}

// ---- tokens ---------------------------------------------------------------

interface Tok {
  kind: "word" | "quoted" | "dot" | "comma" | "semi" | "lparen" | "skip" | "other";
  /** Unquoted text of words and quoted names. */
  value: string;
  from: number;
  to: number;
  /** A string or comment still open at `to` (a line comment always is). */
  open?: boolean;
}

const WORD = /[\p{L}_@#][\p{L}\p{N}_@#$]*/uy;

function tokenize(sql: string): Tok[] {
  const out: Tok[] = [];
  let i = 0;
  const n = sql.length;
  while (i < n) {
    const c = sql[i];
    const from = i;
    if (/\s/.test(c)) {
      i++;
    } else if (c === "-" && sql[i + 1] === "-") {
      const end = sql.indexOf("\n", i);
      i = end < 0 ? n : end;
      out.push({ kind: "skip", value: "", from, to: i, open: true });
    } else if (c === "/" && sql[i + 1] === "*") {
      const end = sql.indexOf("*/", i + 2);
      i = end < 0 ? n : end + 2;
      out.push({ kind: "skip", value: "", from, to: i, open: end < 0 });
    } else if (c === "'" || ((c === "N" || c === "n") && sql[i + 1] === "'")) {
      i = c === "'" ? i + 1 : i + 2;
      let open = true;
      while (i < n) {
        if (sql[i] === "'") {
          if (sql[i + 1] === "'") i += 2;
          else {
            i++;
            open = false;
            break;
          }
        } else i++;
      }
      out.push({ kind: "skip", value: "", from, to: i, open });
    } else if (c === "[" || c === '"') {
      const close = c === "[" ? "]" : '"';
      let value = "";
      i++;
      while (i < n) {
        if (sql[i] === close) {
          if (sql[i + 1] === close) {
            value += close;
            i += 2;
          } else {
            i++;
            break;
          }
        } else value += sql[i++];
      }
      out.push({ kind: "quoted", value, from, to: i });
    } else {
      WORD.lastIndex = i;
      const m = WORD.exec(sql);
      if (m) {
        i += m[0].length;
        out.push({ kind: "word", value: m[0], from, to: i });
      } else {
        i++;
        const kind = c === "." ? "dot" : c === "," ? "comma" : c === ";" ? "semi" : c === "(" ? "lparen" : "other";
        out.push({ kind, value: c, from, to: i });
      }
    }
  }
  return out;
}

// ---- batches ----------------------------------------------------------------

/** Lexical state at the start of a line, as in `split_batches` (Rust). */
type LineState = "normal" | "string" | "bracket" | "quoted" | number;

/** The state at the end of `line`; a number is the depth of nested block comments. */
function scanLine(line: string, state: LineState): LineState {
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (typeof state === "number") {
      if (c === "*" && line[i + 1] === "/") {
        i++;
        state = state === 1 ? "normal" : state - 1;
      } else if (c === "/" && line[i + 1] === "*") {
        i++;
        state++;
      }
    } else if (state === "normal") {
      if (c === "'") state = "string";
      else if (c === "[") state = "bracket";
      else if (c === '"') state = "quoted";
      else if (c === "/" && line[i + 1] === "*") {
        i++;
        state = 1;
      } else if (c === "-" && line[i + 1] === "-") return "normal";
    } else if ((state === "string" && c === "'") || (state === "bracket" && c === "]") || (state === "quoted" && c === '"')) {
      state = "normal";
    }
  }
  return state;
}

/** `GO` alone on a line: any case, blanks around, an optional trailing `--` comment; `GO 5` is not one. */
function isGoLine(line: string): boolean {
  const t = line.trim();
  if (t.slice(0, 2).toUpperCase() !== "GO") return false;
  const rest = t.slice(2);
  return rest === "" || rest.startsWith("--") || (/^\s/.test(rest) && rest.trimStart().startsWith("--"));
}

const SPECIAL = /['"[/*-]/;

/**
 * The `[start, end)` text range of the batch around `cursor`: batches are separated by `GO` lines, which
 * belong to none (a cursor on one gets only that line). Only this range needs tokenizing.
 */
export function batchRange(sql: string, cursor: number): [number, number] {
  let start = 0;
  let state: LineState = "normal";
  let at = 0;
  while (at <= sql.length) {
    const nl = sql.indexOf("\n", at);
    const lineEnd = nl < 0 ? sql.length : nl;
    const line = sql.slice(at, lineEnd);
    if (state === "normal" && isGoLine(line)) {
      // A cursor on the GO line gets just that line, so a `--` comment after GO is still seen.
      if (at <= cursor && cursor <= lineEnd) return [at, lineEnd];
      // The batch ends where this line starts (keeping the newline before it out of the batch).
      if (at > cursor) return [start, Math.max(start, at - 1)];
      start = lineEnd + 1;
    } else if (state !== "normal" || SPECIAL.test(line)) state = scanLine(line, state);
    if (nl < 0) break;
    at = nl + 1;
  }
  return [Math.min(start, sql.length), sql.length];
}

/** Tokens of the batch around `cursor`, with offsets into `sql`. */
function tokensAround(sql: string, cursor: number): Tok[] {
  const [from, to] = batchRange(sql, cursor);
  const toks = tokenize(sql.slice(from, to));
  if (from > 0) for (const t of toks) (t.from += from), (t.to += from);
  return toks;
}

/** Tokens of the statement around `cursor` in `toks`, which are of one batch: statements end at `;`. */
function statementAt(toks: Tok[], cursor: number): Tok[] {
  let start = 0;
  let end = toks.length;
  for (let i = 0; i < toks.length; i++) {
    const t = toks[i];
    if (t.kind !== "semi") continue;
    if (t.to <= cursor) start = i + 1;
    else {
      end = i;
      break;
    }
  }
  return toks.slice(start, end);
}

const isName = (t: Tok | undefined) =>
  !!t && (t.kind === "quoted" || (t.kind === "word" && !RESERVED.has(t.value.toUpperCase()) && t.value.toUpperCase() !== "APPLY"));

/** The unique schema that has a table or view `name`, else `dbo`. */
function resolveSchema(name: string, meta?: CompletionMetadata): string {
  if (!meta) return "dbo";
  const lower = name.toLowerCase();
  const schemas = new Set(
    meta.objects.filter((o) => o.kind !== "procedure" && o.name.toLowerCase() === lower).map((o) => o.schema),
  );
  return schemas.size === 1 ? [...schemas][0] : "dbo";
}

/**
 * Aliases of the tables, views, synonyms and schema-qualified table-valued functions in the `FROM`, `JOIN` and `APPLY` clauses of the statement around `cursor`,
 * as unquoted `schema.name`. A name without a schema takes the only schema in `meta` that has it,
 * else `dbo`. When an alias repeats, the one nearest the cursor wins.
 */
export function aliasMap(sql: string, cursor: number, meta?: CompletionMetadata): Record<string, string> {
  return aliasesIn(tokensAround(sql, cursor), cursor, meta);
}

/** `aliasMap` over the tokens of the batch around `cursor`. */
function aliasesIn(batch: Tok[], cursor: number, meta?: CompletionMetadata): Record<string, string> {
  const toks = statementAt(
    batch.filter((t) => t.kind !== "skip"),
    cursor,
  );
  const found = new Map<string, { alias: string; target: string; dist: number }>();
  for (let i = 0; i < toks.length; i++) {
    const kw = toks[i].kind === "word" ? toks[i].value.toUpperCase() : "";
    if (kw !== "FROM" && kw !== "JOIN" && kw !== "APPLY") continue;
    let j = i + 1;
    for (;;) {
      // schema.name, db.schema.name or db..name, or a schema-qualified function call; derived tables are skipped.
      const parts: string[] = [];
      while (true) {
        const t = toks[j];
        if (t && (t.kind === "word" || t.kind === "quoted") && (parts.length === 0 || toks[j - 1].kind === "dot")) {
          parts.push(t.value);
          j++;
        } else if (t?.kind === "dot") {
          if (toks[j - 1]?.kind === "dot") parts.push("");
          j++;
        } else break;
      }
      if (parts.length === 0) break;
      if (toks[j]?.kind === "lparen") {
        // A table-valued function needs a schema; `STRING_SPLIT(...)` and the like have no columns here.
        if (parts.length < 2) break;
        let depth = 0;
        for (; j < toks.length; j++) {
          if (toks[j].kind === "lparen") depth++;
          else if (toks[j].kind === "other" && toks[j].value === ")" && --depth === 0) break;
        }
        if (j >= toks.length) break;
        j++;
      }
      const name = parts[parts.length - 1];
      const schema = parts.length > 1 && parts[parts.length - 2] !== "" ? parts[parts.length - 2] : resolveSchema(name, meta);
      if (toks[j]?.kind === "word" && toks[j].value.toUpperCase() === "AS") j++;
      const a = toks[j];
      if (isName(a)) {
        const dist = Math.abs(a.from - cursor);
        const key = a.value.toLowerCase();
        const prev = found.get(key);
        if (!prev || dist < prev.dist) found.set(key, { alias: a.value, target: `${schema}.${name}`, dist });
        j++;
      }
      if (toks[j]?.kind !== "comma") break;
      j++;
    }
  }
  return Object.fromEntries([...found.values()].map((f) => [f.alias, f.target]));
}

// ---- completion -------------------------------------------------------------

const TYPED = /(?:\[(?:[^\]]|\]\])*|[\p{L}\p{N}_@#$]*)$/u;
const QUALIFIER = /(?:\[(?:[^\]]|\]\])*\]|"(?:[^"]|"")*"|[\p{L}_@#][\p{L}\p{N}_@#$]*)$/u;
const unquote = (s: string) =>
  s.startsWith("[") ? s.slice(1, -1).replaceAll("]]", "]") : s.startsWith('"') ? s.slice(1, -1).replaceAll('""', '"') : s;

/** The dotted names before `end` (`a.b.` gives `["a", "b"]`). */
function qualifierBefore(text: string, end: number): string[] {
  const parts: string[] = [];
  let at = end;
  while (at > 0 && text[at - 1] === "." && parts.length < 3) {
    const m = QUALIFIER.exec(text.slice(Math.max(0, at - 1 - 256), at - 1));
    if (!m) break;
    parts.unshift(unquote(m[0]));
    at = at - 1 - m[0].length;
  }
  return parts;
}

const KIND_TYPE: Record<DbObject["kind"], string> = {
  table: "class",
  view: "interface",
  procedure: "function",
  function: "method",
  tableFunction: "method",
  synonym: "type",
};

/** Completions for `meta`, the cached metadata of the tab's database. */
export function completionSource(meta: CompletionMetadata): CompletionSource {
  const columns = new Map(Object.entries(meta.columns).map(([k, v]) => [k.toLowerCase(), v]));
  const schemas = new Set(meta.schemas.map((s) => s.toLowerCase()));
  const relations = meta.objects.filter((o) => o.kind !== "procedure");

  return (ctx: CompletionContext): CompletionResult | null => {
    // An IME is composing: the text under the cursor is not final, so nothing opens.
    if (ctx.view?.composing || ctx.view?.compositionStarted) return null;
    const text = ctx.state.doc.toString();
    const pos = ctx.pos;
    const toks = tokensAround(text, pos);
    if (toks.some((t) => t.kind === "skip" && t.from < pos && (pos < t.to || (t.open && pos === t.to)))) return null;

    const typed = TYPED.exec(text.slice(Math.max(0, pos - 256), pos))?.[0] ?? "";
    const from = pos - typed.length;
    const bracketed = typed.startsWith("[");
    const name = (s: string) => (bracketed ? quoteIdent(s) : sqlName(s));
    const option = (label: string, type: string, detail?: string, apply?: string): Completion => {
      const shown = bracketed ? quoteIdent(label) : label;
      const insert = apply ?? name(label);
      return { label: shown, type, detail, ...(insert !== shown ? { apply: insert } : {}) };
    };
    const columnOptions = (key: string) => (columns.get(key.toLowerCase()) ?? []).map((c) => option(c, "property"));
    const validFor = bracketed ? /^\[(?:[^\]]|\]\])*$/u : /^[\p{L}\p{N}_@#$]*$/u;
    const result = (options: Completion[]): CompletionResult | null =>
      options.length > 0 ? { from, options, validFor } : null;

    const qual = qualifierBefore(text, from);
    if (qual.length === 1) {
      const [q] = qual;
      const aliases = aliasesIn(toks, pos, meta);
      const alias = Object.keys(aliases).find((a) => a.toLowerCase() === q.toLowerCase());
      if (alias) return result(columnOptions(aliases[alias]));
      if (schemas.has(q.toLowerCase())) {
        const lower = q.toLowerCase();
        return result(
          meta.objects.filter((o) => o.schema.toLowerCase() === lower).map((o) => option(o.name, KIND_TYPE[o.kind], o.kind)),
        );
      }
      return result(columnOptions(`${resolveSchema(q, meta)}.${q}`));
    }
    if (qual.length > 1) return result(columnOptions(`${qual[qual.length - 2]}.${qual[qual.length - 1]}`));

    if (typed === "" && !ctx.explicit) return null;
    const options: Completion[] = [];
    if (!bracketed) for (const k of KEYWORDS) options.push({ label: k, type: "keyword" });
    for (const s of meta.schemas) options.push(option(s, "namespace", "schema"));
    for (const o of relations) {
      // A scalar function must be called with a two-part name, so functions are never inserted bare.
      const isFunction = o.kind === "function" || o.kind === "tableFunction";
      const apply = o.schema.toLowerCase() === "dbo" && !isFunction ? undefined : `${sqlName(o.schema)}.${name(o.name)}`;
      options.push(option(o.name, KIND_TYPE[o.kind], o.schema, apply));
    }
    return result(options);
  };
}

// ---- metadata cache -----------------------------------------------------------

/** Completion metadata per (connection, database), fetched once until invalidated. Failures are not kept. */
export class MetadataCache {
  private entries = new Map<string, Promise<CompletionMetadata>>();
  constructor(private fetch: (connectionId: string, database: string) => Promise<CompletionMetadata>) {}

  get(connectionId: string, database: string): Promise<CompletionMetadata> {
    const key = JSON.stringify([connectionId, database]);
    let p = this.entries.get(key);
    if (!p) {
      p = this.fetch(connectionId, database);
      this.entries.set(key, p);
      const mine = p;
      p.catch(() => {
        if (this.entries.get(key) === mine) this.entries.delete(key);
      });
    }
    return p;
  }

  invalidate(connectionId: string, database: string): void {
    this.entries.delete(JSON.stringify([connectionId, database]));
  }
}

let shared: MetadataCache | null = null;

/** The app-wide cache; a database is fetched again after it is re-indexed. */
export function sharedMetadata(): MetadataCache {
  if (!shared) {
    const cache = (shared = new MetadataCache(ipc.completionMetadata));
    ipc
      .onAppEvent((e) => {
        if (e.kind === "indexProgress" && e.done) cache.invalidate(e.connectionId, e.database);
      })
      .catch(() => {});
  }
  return shared;
}
