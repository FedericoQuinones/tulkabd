import { expect, test, vi } from "vitest";
import type { ViewSpec } from "../ipc";
import { EMPTY_VIEW, isFiltered, nextSort, ResultView, resultView, withColumnFilter } from "./view";

const flush = () => new Promise((r) => setTimeout(r, 0));

test("nextSort goes ascending, descending, then back to the original order", () => {
  const asc = nextSort(null, 2);
  expect(asc).toEqual({ column: 2, descending: false });
  const desc = nextSort(asc, 2);
  expect(desc).toEqual({ column: 2, descending: true });
  expect(nextSort(desc, 2)).toBeNull();
});

test("nextSort on another column starts ascending", () => {
  expect(nextSort({ column: 2, descending: true }, 0)).toEqual({ column: 0, descending: false });
  expect(nextSort({ column: 2, descending: false }, 1)).toEqual({ column: 1, descending: false });
});

test("a column keeps one filter, the new one replaces it, null clears it", () => {
  const a = withColumnFilter([], 1, { kind: "isNull", column: 1 });
  expect(a).toEqual([{ kind: "isNull", column: 1 }]);
  const b = withColumnFilter([...a, { kind: "notNull", column: 3 }], 1, { kind: "equals", column: 1, text: "x" });
  expect(b).toEqual([
    { kind: "notNull", column: 3 },
    { kind: "equals", column: 1, text: "x" },
  ]);
  expect(withColumnFilter(b, 3, null)).toEqual([{ kind: "equals", column: 1, text: "x" }]);
});

test("isFiltered is about filters, not the sort", () => {
  expect(isFiltered(EMPTY_VIEW)).toBe(false);
  expect(isFiltered({ ...EMPTY_VIEW, sort: { column: 0, descending: false } })).toBe(false);
  expect(isFiltered({ ...EMPTY_VIEW, text: "" })).toBe(false);
  expect(isFiltered({ ...EMPTY_VIEW, text: "a" })).toBe(true);
  expect(isFiltered({ ...EMPTY_VIEW, columns: [{ kind: "isNull", column: 0 }] })).toBe(true);
});

function deferred() {
  const calls: { spec: ViewSpec; resolve: (n: number) => void; reject: (e: unknown) => void }[] = [];
  const send = vi.fn(
    (spec: ViewSpec) =>
      new Promise<number>((resolve, reject) => {
        calls.push({ spec, resolve, reject });
      }),
  );
  return { calls, send };
}

test("calls are sent one at a time and only the newest waiting view is sent next", async () => {
  const { calls, send } = deferred();
  const v = new ResultView(send);
  const a = { ...EMPTY_VIEW, text: "a" };
  const ab = { ...EMPTY_VIEW, text: "ab" };
  const abc = { ...EMPTY_VIEW, text: "abc" };
  v.set(a);
  v.set(ab);
  v.set(abc);
  expect(send).toHaveBeenCalledTimes(1);
  expect(v.spec).toEqual(abc);
  expect(v.pending).toBe(true);
  calls[0].resolve(10);
  await flush();
  // "ab" was replaced by "abc" while "a" ran.
  expect(send).toHaveBeenCalledTimes(2);
  expect(calls[1].spec).toEqual(abc);
  // The grid keeps its rows until the last view is in.
  expect(v.generation).toBe(0);
  calls[1].resolve(3);
  await flush();
  expect(v.pending).toBe(false);
  expect(v.visible).toBe(3);
  expect(v.generation).toBe(1);
  expect(v.applied).toEqual(abc);
});

test("an empty view means all rows again", async () => {
  const send = vi.fn(async () => 20);
  const v = new ResultView(send);
  v.set({ ...EMPTY_VIEW, text: "x" });
  await flush();
  expect(v.visible).toBe(20);
  v.set(EMPTY_VIEW);
  await flush();
  expect(v.visible).toBeNull();
  expect(v.generation).toBe(2);
});

test("a failed call goes back to the view the backend has and tells the listeners", async () => {
  const { calls, send } = deferred();
  const v = new ResultView(send);
  const errors: (string | null)[] = [];
  v.subscribe((e) => errors.push(e));
  const sorted = { ...EMPTY_VIEW, sort: { column: 0, descending: false } };
  v.set(sorted);
  calls[0].resolve(5);
  await flush();
  v.set({ ...sorted, text: "q" });
  calls[1].reject("busy");
  await flush();
  expect(v.spec).toEqual(sorted);
  expect(v.applied).toEqual(sorted);
  expect(v.pending).toBe(false);
  expect(errors.at(-1)).toBe("busy");
  expect(v.generation).toBe(1);
});

test("unsubscribe stops the notifications", async () => {
  const v = new ResultView(async () => 1);
  const fn = vi.fn();
  const off = v.subscribe(fn);
  off();
  v.set({ ...EMPTY_VIEW, text: "x" });
  await flush();
  expect(fn).not.toHaveBeenCalled();
});

test("resultView keeps one view per result of a run and starts over on a new run", () => {
  const owner = {};
  const send = async () => 0;
  const a = resultView(owner, 1, 0, send);
  expect(resultView(owner, 1, 0, send)).toBe(a);
  expect(resultView(owner, 1, 1, send)).not.toBe(a);
  const next = resultView(owner, 2, 0, send);
  expect(next).not.toBe(a);
  expect(next.spec).toEqual(EMPTY_VIEW);
});

test("a view of an older run never sends what was still waiting", async () => {
  const { calls, send } = deferred();
  const owner = {};
  const old = resultView(owner, 1, 0, send);
  old.set({ ...EMPTY_VIEW, text: "a" });
  old.set({ ...EMPTY_VIEW, text: "ab" });
  resultView(owner, 2, 0, send);
  calls[0].resolve(1);
  await flush();
  expect(send).toHaveBeenCalledTimes(1);
  expect(old.pending).toBe(false);
});
