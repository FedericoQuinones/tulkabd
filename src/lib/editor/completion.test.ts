import { expect, test, vi } from "vitest";
import { CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import { EditorState } from "@codemirror/state";
import type { CompletionMetadata } from "../ipc";
import { aliasMap, batchRange, completionSource, MetadataCache } from "./completion";

const meta: CompletionMetadata = {
  schemas: ["dbo", "ventas", "we]ird"],
  objects: [
    { schema: "dbo", name: "Pedidos", kind: "table" },
    { schema: "ventas", name: "Pedidos_Año", kind: "table" },
    { schema: "ventas", name: "Clientes", kind: "table" },
    { schema: "dbo", name: "Order Details", kind: "table" },
    { schema: "dbo", name: "Order", kind: "view" },
    { schema: "we]ird", name: "a]b", kind: "table" },
    { schema: "dbo", name: "CerrarMes", kind: "procedure" },
    { schema: "dbo", name: "fnTotal", kind: "function" },
    { schema: "dbo", name: "fnRows", kind: "tableFunction" },
    { schema: "ventas", name: "synClientes", kind: "synonym" },
  ],
  columns: {
    "dbo.Pedidos": ["Id", "Cliente", "Total"],
    "ventas.Pedidos_Año": ["Id", "Año"],
    "ventas.Clientes": ["Id", "Nombre"],
    "dbo.Order Details": ["Unit Price"],
  },
};

/** Completes at `|` in `doc`. */
async function complete(doc: string, explicit = false): Promise<CompletionResult | null> {
  const pos = doc.indexOf("|");
  const state = EditorState.create({ doc: doc.slice(0, pos) + doc.slice(pos + 1) });
  return completionSource(meta)(new CompletionContext(state, pos, explicit));
}
const labels = (r: CompletionResult | null) => r?.options.map((o) => o.label) ?? [];
const option = (r: CompletionResult | null, label: string) => r?.options.find((o) => o.label === label);

test("aliasMap maps FROM and JOIN aliases of the statement around the cursor", () => {
  expect(aliasMap("SELECT p. FROM dbo.Pedidos p JOIN ventas.Pedidos_Año AS a ON 1=1", 9)).toEqual({
    p: "dbo.Pedidos",
    a: "ventas.Pedidos_Año",
  });
});

test("aliasMap reads quoted names, comma lists and skips other statements", () => {
  const sql = "SELECT 1 FROM x q;\nSELECT o. FROM [dbo].[Order Details] o, Clientes c WHERE 1=1";
  expect(aliasMap(sql, sql.indexOf("o.") + 2)).toEqual({ o: "dbo.Order Details", c: "dbo.Clientes" });
});

test("a table without a schema takes the only schema that has it, or dbo", () => {
  const sql = "SELECT * FROM Clientes c JOIN Pedidos p ON 1=1 JOIN Nada n ON 1=1";
  expect(aliasMap(sql, 0, meta)).toEqual({ c: "ventas.Clientes", p: "dbo.Pedidos", n: "dbo.Nada" });
});

test("keywords after a table are not aliases", () => {
  expect(aliasMap("SELECT * FROM dbo.Pedidos WHERE Id = 1", 0)).toEqual({});
});

test("after p. it offers the columns of the aliased table", async () => {
  const r = await complete("SELECT p.| FROM dbo.Pedidos p JOIN ventas.Pedidos_Año AS a ON 1=1");
  expect(labels(r)).toEqual(["Id", "Cliente", "Total"]);
  expect(labels(await complete("SELECT a.A| FROM dbo.Pedidos p JOIN ventas.Pedidos_Año AS a ON 1=1"))).toEqual(["Id", "Año"]);
});

test("after a table name or schema.table it offers columns; after a schema, its objects", async () => {
  expect(labels(await complete("SELECT Pedidos.| FROM Pedidos"))).toEqual(["Id", "Cliente", "Total"]);
  expect(labels(await complete("SELECT ventas.Clientes.| FROM ventas.Clientes"))).toEqual(["Id", "Nombre"]);
  expect(labels(await complete("SELECT * FROM ventas.|"))).toEqual(["Pedidos_Año", "Clientes", "synClientes"]);
});

test("names that need brackets are inserted quoted", async () => {
  const r = await complete("SELECT * FROM Ord|");
  expect(option(r, "Order Details")?.apply).toBe("[Order Details]");
  expect(option(r, "Order")?.apply).toBe("[Order]");
  expect(option(r, "Pedidos")).toMatchObject({ label: "Pedidos", type: "class", detail: "dbo" });
  expect(option(r, "Pedidos")?.apply).toBeUndefined();
  const año = option(await complete("SELECT * FROM Ped|"), "Pedidos_Año");
  expect(año?.apply).toBe("ventas.Pedidos_Año");
  expect(option(await complete("SELECT * FROM a|"), "a]b")?.apply).toBe("[we]]ird].[a]]b]");
  expect(option(await complete("SELECT o.| FROM dbo.[Order Details] o"), "Unit Price")?.apply).toBe("[Unit Price]");
});

test("a typed [ keeps the bracketed form", async () => {
  const r = await complete("SELECT * FROM [Order D|");
  expect(r?.from).toBe("SELECT * FROM ".length);
  const o = option(r, "[Order Details]");
  expect(o).toBeDefined();
  expect(o?.apply).toBeUndefined();
  // Bracketed form: every option is bracketed and no bare keyword is offered.
  expect(labels(r).every((l) => l.startsWith("["))).toBe(true);
  expect(labels(r)).toContain("[Order]");
});

test("plain words offer keywords, schemas, tables and views", async () => {
  const r = await complete("SEL|");
  expect(labels(r)).toContain("SELECT");
  const all = labels(await complete("|", true));
  for (const l of ["dbo", "ventas", "Pedidos", "Order"]) expect(all).toContain(l);
  expect(all).not.toContain("CerrarMes");
  // Functions and synonyms sit where tables do; a schema-qualified one takes its schema.
  expect(all).toContain("fnTotal");
  expect(all).toContain("synClientes");
  expect(option(await complete("|", true), "synClientes")?.apply).toBe("ventas.synClientes");
  // A scalar function needs a two-part name, so even dbo ones are inserted with their schema.
  expect(option(await complete("|", true), "fnTotal")?.apply).toBe("dbo.fnTotal");
  expect(option(await complete("|", true), "fnRows")?.apply).toBe("dbo.fnRows");
  expect(option(await complete("|", true), "Pedidos")?.apply).toBeUndefined();
  expect(labels(await complete("ventas.|"))).toContain("synClientes");
  expect(labels(await complete("dbo.|"))).toContain("fnTotal");
});

test("nothing inside strings or comments, or with no word unless asked", async () => {
  expect(await complete("SELECT 'Ped|'")).toBeNull();
  expect(await complete("-- Ped|")).toBeNull();
  expect(await complete("/* p.| */")).toBeNull();
  expect(await complete("SELECT |")).toBeNull();
});

test("the metadata cache fetches once per connection and database until it is invalidated", async () => {
  const fetch = vi.fn(async (c: string, d: string) => ({ ...meta, schemas: [`${c}/${d}`] }));
  const cache = new MetadataCache(fetch);
  const a = await cache.get("c1", "Ventas");
  expect(await cache.get("c1", "Ventas")).toBe(a);
  await cache.get("c1", "Hist");
  expect(fetch).toHaveBeenCalledTimes(2);
  cache.invalidate("c1", "Ventas");
  expect(await cache.get("c1", "Ventas")).not.toBe(a);
  expect(fetch).toHaveBeenCalledTimes(3);
});

test("GO separates batches by the same rules as the Rust split_batches", () => {
  const at = (sql: string) => aliasMap(sql, sql.length);
  // A GO alone on a line (any case, blanks, trailing comment) ends the batch.
  expect(at("SELECT 1 FROM a x\nGO\nSELECT 2 FROM b y")).toEqual({ y: "dbo.b" });
  expect(at("SELECT 1 FROM a x\n  go  \nSELECT 2 FROM b y")).toEqual({ y: "dbo.b" });
  expect(at("SELECT 1 FROM a x\nGo -- done\nSELECT 2 FROM b y")).toEqual({ y: "dbo.b" });
  expect(at("SELECT 1 FROM a x\nGO--done\nSELECT 2 FROM b y")).toEqual({ y: "dbo.b" });
  // GO with a count, or with other text on its line, is not a separator.
  expect(at("SELECT 1 FROM a x\nGO 5\nSELECT 2 FROM b y")).toEqual({ x: "dbo.a", y: "dbo.b" });
  expect(at("SELECT 1 FROM a x GO\nSELECT 2 FROM b y")).toEqual({ x: "dbo.a", y: "dbo.b" });
  expect(at("SELECT 1 FROM a x\nWHERE go = 1")).toEqual({ x: "dbo.a" });
  // GO inside a string or a block comment is no separator.
  expect(at("SELECT 'a\nGO\n' FROM a x")).toEqual({ x: "dbo.a" });
  expect(at("/*\nGO\n*/ SELECT 1 FROM a x")).toEqual({ x: "dbo.a" });
});

test("only the batch around the cursor is looked at", async () => {
  const doc = "SELECT 1 FROM Pedidos p\nGO\nSELECT p.| FROM ventas.Clientes p\nGO\nSELECT 3 FROM Pedidos p";
  expect(labels(await complete(doc))).toEqual(["Id", "Nombre"]);
  const sql = "SELECT 'a\nGO\n' FROM a x\nGO\nSELECT 1 FROM b y";
  expect(batchRange(sql, 0)).toEqual([0, sql.indexOf("\nGO\nSELECT 1")]);
  expect(batchRange(sql, sql.length)).toEqual([sql.indexOf("SELECT 1"), sql.length]);
  // A cursor on a GO line gets only that line, never the next batch.
  const go = "SELECT 1\nGO\nSELECT 2";
  expect(batchRange(go, go.indexOf("GO") + 1)).toEqual([9, 11]);
  expect(batchRange("GO\nGO\nSELECT 1", 4)).toEqual([3, 5]);
  expect(batchRange("GO\nSELECT 1", 1)).toEqual([0, 2]);
  expect(await complete("SELECT 1\nGO -- hello wor|")).toBeNull();
  // A GO inside a string left open is part of the string, so there is no second batch.
  expect(await complete("SELECT 'x\nGO\nSEL|")).toBeNull();
});

test("nothing opens while an IME is composing", () => {
  const state = EditorState.create({ doc: "SEL" });
  const run = (view: object) =>
    completionSource(meta)(new CompletionContext(state, 3, false, view as never)) as CompletionResult | null;
  expect(labels(run({ composing: false, compositionStarted: false }))).toContain("SELECT");
  expect(run({ composing: true, compositionStarted: true })).toBeNull();
  expect(run({ composing: false, compositionStarted: true })).toBeNull();
});

test("a failed fetch is not cached", async () => {
  const fetch = vi.fn().mockRejectedValueOnce("not_connected").mockResolvedValueOnce(meta);
  const cache = new MetadataCache(fetch);
  await expect(cache.get("c1", "Ventas")).rejects.toBe("not_connected");
  expect(await cache.get("c1", "Ventas")).toBe(meta);
});
