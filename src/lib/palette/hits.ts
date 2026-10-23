// Pure parts of the object palette: grouping, match highlighting and keys.
import type { PaletteHit, SavedQueryHit } from "../ipc";

export interface PaletteGroup {
  key: string;
  /** Folder names from the root, then the connection and the database. */
  path: string[];
  connected: boolean;
  hits: PaletteHit[];
}

/** Groups hits by connection and database; groups and their hits keep the ranking order. */
export function groupHits(hits: PaletteHit[]): PaletteGroup[] {
  const groups = new Map<string, PaletteGroup>();
  for (const h of hits) {
    const key = JSON.stringify([h.connectionId, h.database]);
    let g = groups.get(key);
    if (!g) {
      g = { key, path: [...h.folderPath, h.connectionName, h.database], connected: h.connected, hits: [] };
      groups.set(key, g);
    }
    g.hits.push(h);
  }
  return [...groups.values()];
}

export interface QueryGroup {
  key: string;
  database: string;
  queries: SavedQueryHit[];
}

/** Groups saved query hits by database ("Saved queries › <database>", plan D6), keeping the ranking order. */
export function groupQueries(queries: SavedQueryHit[]): QueryGroup[] {
  const groups = new Map<string, QueryGroup>();
  for (const q of queries) {
    let g = groups.get(q.database);
    if (!g) groups.set(q.database, (g = { key: JSON.stringify(["q", q.database]), database: q.database, queries: [] }));
    g.queries.push(q);
  }
  return [...groups.values()];
}

/** A row of the palette: a saved query or a database object. */
export type PaletteItem = { kind: "query"; query: SavedQueryHit } | { kind: "object"; hit: PaletteHit };

export interface Segment {
  text: string;
  hit: boolean;
}

/** Splits `text` into runs of matched and unmatched characters; `indices` are code point positions. */
export function highlight(text: string, indices: number[]): Segment[] {
  const marked = new Set(indices);
  const out: Segment[] = [];
  Array.from(text).forEach((ch, i) => {
    const hit = marked.has(i);
    const last = out[out.length - 1];
    if (last && last.hit === hit) last.text += ch;
    else out.push({ text: ch, hit });
  });
  return out;
}

export type PaletteAction = "open" | "selectTop" | "definition";
export type PaletteKey = "up" | "down" | "close" | PaletteAction;

/**
 * Maps a keydown in the palette input. ArrowRight opens the definition only with the
 * caret at the end of the input; elsewhere it moves the caret.
 */
export function paletteKey(e: KeyboardEvent, isMac: boolean, caretAtEnd: boolean): PaletteKey | null {
  const mod = isMac ? e.metaKey : e.ctrlKey;
  if (e.altKey || e.shiftKey) return null;
  switch (e.key) {
    case "ArrowDown":
      return mod ? null : "down";
    case "ArrowUp":
      return mod ? null : "up";
    case "Enter":
      return mod ? "selectTop" : "open";
    case "ArrowRight":
      return !mod && caretAtEnd ? "definition" : null;
    case "Escape":
      return "close";
  }
  return null;
}

/** Whether an action applies to an object kind: Enter opens anything, Select top 10 needs rows, definitions need code. */
export function canRun(action: PaletteAction, kind: PaletteHit["object"]["kind"]): boolean {
  if (action === "selectTop") return kind !== "procedure";
  if (action === "definition") return kind !== "table";
  return true;
}
