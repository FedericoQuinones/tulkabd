import type { DbObject, ObjectKind } from "./ipc";

/**
 * The kind whose actions an object gets: a synonym acts as the object it points to, and as nothing
 * but a synonym while that is unknown (missing, in an unreadable database, on a linked server).
 */
export function effectiveKind(o: Pick<DbObject, "kind" | "target">): ObjectKind {
  return o.kind === "synonym" ? (o.target ?? "synonym") : o.kind;
}

/**
 * Whether `SELECT TOP 10 *` works on it; a function needs arguments and a procedure is run, not read.
 * A synonym whose target is unknown (linked server, unreadable database) is left to the server.
 */
export function canSelectTop(o: Pick<DbObject, "kind" | "target">): boolean {
  const kind = effectiveKind(o);
  return kind === "table" || kind === "view" || kind === "synonym";
}

/** Whether its row expands into columns. */
export function hasColumns(o: Pick<DbObject, "kind" | "target">): boolean {
  const kind = effectiveKind(o);
  return kind === "table" || kind === "view" || kind === "tableFunction";
}

/** Whether it is used by name as a function: called with arguments, so never inserted bare. */
export function isFunctionLike(o: Pick<DbObject, "kind" | "target">): boolean {
  const kind = effectiveKind(o);
  return kind === "function" || kind === "tableFunction";
}
