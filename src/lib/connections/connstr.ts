// Filling a connection from a pasted connection string. The parsing itself is
// done by the backend (`parse_connection_string`); this maps its result and
// error codes to the form and the user's text.
import { t } from "../i18n/t";
import * as ipc from "../ipc";
import type { DatabaseScope, ParsedConnection } from "../ipc";

/** Longest text a paste on the tree sends to the backend; anything longer is not a connection string. */
export const MAX_PASTE = 4096;

const CODES = ["empty", "no_server", "bad_port", "unsupported_auth", "unrecognized"];

/** The message for a `parse_connection_string` error code. */
export function connStrErrorText(e: unknown): string {
  const code = String(e);
  if (code.startsWith("bad_value:")) {
    return t("connection.connstr.errors.bad_value", { key: code.slice("bad_value:".length) });
  }
  return t(`connection.connstr.errors.${CODES.includes(code) ? code : "unrecognized"}`);
}

/**
 * "Ignored: Application Name, Pooling and 1 unknown setting", or `null` when
 * nothing was ignored. Unknown keys are never named.
 */
export function ignoredText(p: ParsedConnection): string | null {
  const parts = [...p.ignoredKeys];
  if (p.unknownKeys === 1) parts.push(t("connection.connstr.unknownOne"));
  else if (p.unknownKeys > 1) parts.push(t("connection.connstr.unknownMany", { count: p.unknownKeys }));
  if (parts.length === 0) return null;
  const last = parts.pop()!;
  const keys = parts.length === 0 ? last : t("connection.connstr.and", { rest: parts.join(", "), last });
  return t("connection.connstr.ignored", { keys });
}

/** Name for a connection filled from a string: `<database> @ <server>`, or the server. */
export function proposeName(p: ParsedConnection): string {
  return p.database ? `${p.database} @ ${p.server}` : p.server;
}

export interface ConnStrFill {
  server: string;
  port: number;
  user: string;
  password: string;
  databases: DatabaseScope;
  encrypt?: boolean;
  trust?: boolean;
  timeout?: number;
}

/**
 * The form values a parsed string sets. Server, port, user, password and
 * databases always change (they belong to the new server; a missing port is
 * 1433, a missing database means All); the options only when the string names them.
 */
export function fillFrom(p: ParsedConnection): ConnStrFill {
  const fill: ConnStrFill = {
    server: p.server,
    port: p.port ?? 1433,
    user: p.user ?? "",
    password: p.password ?? "",
    databases: p.database ? { kind: "selected", names: [p.database] } : { kind: "all" },
  };
  if (p.encrypt !== null) fill.encrypt = p.encrypt;
  if (p.trustServerCertificate !== null) fill.trust = p.trustServerCertificate;
  if (p.connectTimeoutS !== null) fill.timeout = p.connectTimeoutS;
  return fill;
}

/**
 * Parses text pasted outside the connection dialog. `null` when it is not a
 * usable connection string: the caller then does nothing and reports nothing.
 */
export async function parsePasted(text: string | null | undefined): Promise<ParsedConnection | null> {
  if (!text || text.trim() === "" || text.length > MAX_PASTE) return null;
  try {
    return await ipc.parseConnectionString(text);
  } catch {
    return null;
  }
}
