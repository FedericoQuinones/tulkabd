import type { TableStructure } from "../ipc";

const MAX_CACHED = 50;
const cache = new Map<string, TableStructure>();
const keyOf = (connectionId: string, database: string, schema: string, name: string) =>
  JSON.stringify([connectionId, database, schema, name]);

export const cachedStructure = (c: string, d: string, s: string, n: string) => cache.get(keyOf(c, d, s, n));

export function cacheStructure(c: string, d: string, s: string, n: string, value: TableStructure) {
  const k = keyOf(c, d, s, n);
  cache.delete(k);
  cache.set(k, value);
  if (cache.size > MAX_CACHED) cache.delete(cache.keys().next().value!);
}

/** Drops what was loaded through a connection: it reconnected, or it was edited or deleted. */
export function forgetStructures(connectionId: string) {
  for (const k of [...cache.keys()]) if ((JSON.parse(k) as string[])[0] === connectionId) cache.delete(k);
}

export function clearStructureCache() {
  cache.clear();
}
