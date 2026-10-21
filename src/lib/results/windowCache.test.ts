import { expect, test, vi } from "vitest";
import type { Value } from "../ipc";
import { WindowCache } from "./windowCache";

const row = (i: number): Value[] => [{ kind: "int", value: String(i) }];
const rowsOf = (offset: number, n: number) => Array.from({ length: n }, (_, i) => row(offset + i));
const flush = () => new Promise((r) => setTimeout(r, 0));

test("requests aligned windows and dedupes in-flight ones", async () => {
  const fetch = vi.fn(async (o: number, l: number) => rowsOf(o, l));
  const c = new WindowCache(fetch);
  c.ensure(150, 420, 1000);
  c.ensure(150, 420, 1000);
  await flush();
  expect(fetch.mock.calls.map((x) => x[0])).toEqual([0, 200, 400]);
  expect(c.get(399)).toEqual(row(399));
  c.ensure(150, 420, 1000);
  await flush();
  expect(fetch).toHaveBeenCalledTimes(3);
});

test("refetches when total grew while the request was in flight", async () => {
  let release!: (r: Value[][]) => void;
  const fetch = vi
    .fn<(o: number, l: number) => Promise<Value[][]>>()
    .mockImplementationOnce(() => new Promise((r) => (release = r)))
    .mockImplementation(async (o) => rowsOf(o, 120));
  const c = new WindowCache(fetch);
  c.ensure(0, 50, 50);
  c.ensure(0, 120, 120); // total grows; first request still in flight, so deduped
  expect(fetch).toHaveBeenCalledTimes(1);
  release(rowsOf(0, 50)); // short response
  await flush();
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(c.get(119)).toEqual(row(119));
});

test("ignores responses after destroy", async () => {
  let release!: (r: Value[][]) => void;
  const onChange = vi.fn();
  const c = new WindowCache(() => new Promise((r) => (release = r)), onChange);
  c.ensure(0, 10, 10);
  c.destroy();
  release(rowsOf(0, 10));
  await flush();
  expect(c.get(0)).toBeUndefined();
  expect(onChange).not.toHaveBeenCalled();
});

test("failed fetch sets the error and retry fetches again", async () => {
  const fetch = vi
    .fn<(o: number, l: number) => Promise<Value[][]>>()
    .mockRejectedValueOnce("boom")
    .mockImplementation(async (o, l) => rowsOf(o, l));
  const c = new WindowCache(fetch);
  c.ensure(0, 10, 10);
  await flush();
  expect(c.error).toBe("boom");
  c.ensure(0, 10, 10); // scrolling alone does not loop on a failing window
  await flush();
  expect(fetch).toHaveBeenCalledTimes(1);
  c.retry();
  await flush();
  expect(c.error).toBeNull();
  expect(c.get(9)).toEqual(row(9));
});

test("a short window with unchanged total is fetched once, not in a loop", async () => {
  const fetch = vi.fn(async (o: number) => rowsOf(o, 3));
  const c = new WindowCache(fetch);
  c.ensure(0, 10, 10);
  await flush();
  await flush();
  expect(fetch).toHaveBeenCalledTimes(1);
  const empty = vi.fn(async () => [] as Value[][]);
  const c2 = new WindowCache(empty);
  c2.ensure(0, 10, 10);
  await flush();
  await flush();
  expect(empty).toHaveBeenCalledTimes(1);
});

test("error stays while another window is failed, and clears after retry succeeds", async () => {
  let failA = true;
  const fetch = vi.fn(async (o: number, l: number) => {
    if (o === 0 && failA) throw "boom";
    return rowsOf(o, l);
  });
  const c = new WindowCache(fetch);
  c.ensure(0, 10, 400);
  await flush();
  expect(c.error).toBe("boom");
  c.ensure(200, 210, 400); // window B succeeds
  await flush();
  expect(c.get(200)).toEqual(row(200));
  expect(c.error).toBe("boom");
  failA = false;
  c.retry();
  await flush();
  expect(c.get(0)).toEqual(row(0));
  expect(c.error).toBeNull();
});

test("keeps at most maxWindows windows, evicting the least recently ensured", async () => {
  const fetch = vi.fn(async (o: number, l: number) => rowsOf(o, l));
  const c = new WindowCache(fetch, undefined, 10, 3);
  for (const start of [0, 10, 20]) {
    c.ensure(start, start + 10, 1000);
    await flush();
  }
  c.ensure(0, 10, 1000); // window 0 becomes the most recent
  await flush();
  c.ensure(30, 40, 1000); // a fourth window evicts window 10, the least recent
  await flush();
  expect(c.get(5)).toEqual(row(5));
  expect(c.get(15)).toBeUndefined();
  expect(c.get(25)).toEqual(row(25));
  expect(c.get(35)).toEqual(row(35));
  expect(fetch).toHaveBeenCalledTimes(4);
  // Scrolling back refetches the evicted window.
  c.ensure(10, 20, 1000);
  await flush();
  expect(fetch).toHaveBeenCalledTimes(5);
  expect(c.get(15)).toEqual(row(15));
  expect(c.get(25)).toBeUndefined();
});

test("never evicts windows of the current range", async () => {
  const fetch = vi.fn(async (o: number, l: number) => rowsOf(o, l));
  const c = new WindowCache(fetch, undefined, 10, 2);
  c.ensure(0, 40, 1000); // four windows in range, more than maxWindows
  await flush();
  for (const r of [0, 15, 25, 39]) expect(c.get(r)).toEqual(row(r));
  c.ensure(100, 110, 1000);
  await flush();
  expect(c.get(105)).toEqual(row(105));
  expect([0, 15, 25, 35].filter((r) => c.get(r) !== undefined)).toHaveLength(1);
});

test("the default bound is 50 windows of 200 rows", async () => {
  const fetch = vi.fn(async (o: number, l: number) => rowsOf(o, l));
  const c = new WindowCache(fetch);
  for (let start = 0; start < 60 * 200; start += 200) {
    c.ensure(start, start + 200, 100_000);
    await flush();
  }
  expect(c.get(0)).toBeUndefined();
  expect(c.get(9 * 200)).toBeUndefined();
  expect(c.get(10 * 200)).toEqual(row(10 * 200));
  expect(c.get(59 * 200 + 199)).toEqual(row(59 * 200 + 199));
});
