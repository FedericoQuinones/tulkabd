import { expect, test } from "vitest";
import { reduceEvent, shortVersion, type PaneBody } from "./reduce";

const body = (): PaneBody => ({ results: [], messages: [], disconnected: false });

test("resultStart then rowCount builds a result", () => {
  const b = body();
  reduceEvent(b, { kind: "resultStart", index: 0, columns: [{ name: "a", typeName: "int" }] }, 0);
  reduceEvent(b, { kind: "rowCount", index: 0, total: 5, truncated: true }, 0);
  expect(b.results).toEqual([{ columns: [{ name: "a", typeName: "int" }], total: 5, truncated: true }]);
});

test("message line is shifted by lineOffset", () => {
  const b = body();
  reduceEvent(b, { kind: "message", messageKind: "error", text: "boom", line: 2 }, 3);
  reduceEvent(b, { kind: "message", messageKind: "info", text: "hi", line: null }, 3);
  expect(b.messages).toEqual([
    { kind: "error", text: "boom", line: 5 },
    { kind: "info", text: "hi", line: null },
  ]);
});

test("rowsAffected becomes an info message", () => {
  const b = body();
  reduceEvent(b, { kind: "rowsAffected", count: 3 }, 0);
  expect(b.messages).toEqual([{ kind: "info", text: "(3 rows affected)", line: null }]);
});

test("disconnected event flags the pane and adds the lost-connection message once", () => {
  const b = body();
  reduceEvent(b, { kind: "disconnected", reason: "x" }, 0);
  reduceEvent(b, { kind: "disconnected", reason: "x" }, 0);
  expect(b.disconnected).toBe(true);
  expect(b.messages).toHaveLength(1);
  expect(b.messages[0].kind).toBe("error");
});

test("batchDone changes nothing", () => {
  const b = body();
  reduceEvent(b, { kind: "batchDone", batch: 1 }, 0);
  expect(b).toEqual(body());
});

test("shortVersion extracts the release year", () => {
  expect(shortVersion("Microsoft SQL Server 2022 (RTM-CU13) (KB5036432) - 16.0.4125.3 (X64)\n\tMar 2024")).toBe("2022");
  expect(shortVersion("weird\nsecond")).toBe("weird");
});
