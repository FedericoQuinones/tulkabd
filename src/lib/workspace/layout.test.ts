import { expect, test } from "vitest";
import { groups, MAX_GROUPS, normalize, removeGroup, resize, split, type Layout } from "./layout";

const g = (id: string): Layout => ({ kind: "group", id });

test("splitting a single group in a row makes a two-child split", () => {
  expect(split(g("g1"), "g1", "row", "g2")).toEqual({
    kind: "split",
    dir: "row",
    children: [g("g1"), g("g2")],
    sizes: [0.5, 0.5],
  });
});

test("splitting again in the same direction adds a sibling with equal thirds", () => {
  const two = split(g("g1"), "g1", "row", "g2")!;
  const three = split(two, "g2", "row", "g3") as Extract<Layout, { kind: "split" }>;
  expect(three.children).toEqual([g("g1"), g("g2"), g("g3")]);
  for (const s of three.sizes) expect(s).toBeCloseTo(1 / 3);
});

test("splitting in the other direction nests", () => {
  const two = split(g("g1"), "g1", "row", "g2")!;
  const nested = split(two, "g2", "column", "g3");
  expect(nested).toEqual({
    kind: "split",
    dir: "row",
    children: [
      g("g1"),
      { kind: "split", dir: "column", children: [g("g2"), g("g3")], sizes: [0.5, 0.5] },
    ],
    sizes: [0.5, 0.5],
  });
});

test("the fifth group is refused", () => {
  let l: Layout = g("g1");
  for (let i = 2; i <= MAX_GROUPS; i++) l = split(l, `g${i - 1}`, "row", `g${i}`)!;
  expect(groups(l)).toHaveLength(4);
  expect(split(l, "g1", "column", "g5")).toBeNull();
});

test("removing a child of a two-way split flattens it", () => {
  const two = split(g("g1"), "g1", "row", "g2")!;
  expect(removeGroup(two, "g1")).toEqual(g("g2"));
});

test("removing from a three-way split renormalizes sizes", () => {
  const three = split(split(g("g1"), "g1", "row", "g2")!, "g2", "row", "g3")!;
  const l = removeGroup(three, "g2") as Extract<Layout, { kind: "split" }>;
  expect(l.children).toEqual([g("g1"), g("g3")]);
  expect(l.sizes[0]).toBeCloseTo(0.5);
  expect(l.sizes[1]).toBeCloseTo(0.5);
});

test("removing a nested group collapses the nested split into its parent", () => {
  const two = split(g("g1"), "g1", "row", "g2")!;
  const nested = split(two, "g2", "column", "g3")!;
  expect(removeGroup(nested, "g3")).toEqual(two);
});

test("removing the only group does nothing", () => {
  expect(removeGroup(g("g1"), "g1")).toEqual(g("g1"));
});

test("resize normalizes sizes that do not sum to 1", () => {
  const two = split(g("g1"), "g1", "row", "g2")!;
  const l = resize(two, [], [1, 3]) as Extract<Layout, { kind: "split" }>;
  expect(l.sizes).toEqual([0.25, 0.75]);
});

test("resize follows an index path to a nested split", () => {
  const nested = split(split(g("g1"), "g1", "row", "g2")!, "g2", "column", "g3")!;
  const l = resize(nested, [1], [0.2, 0.8]) as Extract<Layout, { kind: "split" }>;
  expect((l.children[1] as Extract<Layout, { kind: "split" }>).sizes).toEqual([0.2, 0.8]);
  expect(l.sizes).toEqual([0.5, 0.5]);
});

test("resize ignores invalid input", () => {
  const two = split(g("g1"), "g1", "row", "g2")!;
  expect(resize(two, [], [1])).toEqual(two);
  expect(resize(two, [], [0, 0])).toEqual(two);
  expect(resize(two, [5], [1, 1])).toEqual(two);
});

test("groups lists ids in reading order", () => {
  const nested = split(split(g("g1"), "g1", "row", "g2")!, "g1", "column", "g3")!;
  expect(groups(nested)).toEqual(["g1", "g3", "g2"]);
});

test("normalize flattens single-child splits and same-direction nesting", () => {
  const messy: Layout = {
    kind: "split",
    dir: "row",
    sizes: [0.5, 0.5],
    children: [
      { kind: "split", dir: "row", sizes: [0.5, 0.5], children: [g("a"), g("b")] },
      { kind: "split", dir: "column", sizes: [1], children: [g("c")] },
    ],
  };
  expect(normalize(messy)).toEqual({
    kind: "split",
    dir: "row",
    sizes: [0.25, 0.25, 0.5],
    children: [g("a"), g("b"), g("c")],
  });
});

test("resize rejects zero, negative and non-finite sizes", () => {
  const two = split(g("g1"), "g1", "row", "g2")!;
  expect(resize(two, [], [1, 0])).toEqual(two);
  expect(resize(two, [], [1, -1])).toEqual(two);
  expect(resize(two, [], [1, Infinity])).toEqual(two);
  expect(resize(two, [], [1, NaN])).toEqual(two);
});

test("removing a group after a degenerate resize keeps sizes finite and summing to 1", () => {
  const three = split(split(g("g1"), "g1", "row", "g2")!, "g2", "row", "g3")!;
  const bad: Layout = { ...(three as Extract<Layout, { kind: "split" }>), sizes: [1, 0, 0] };
  const l = removeGroup(bad, "g1") as Extract<Layout, { kind: "split" }>;
  expect(l.sizes.every(Number.isFinite)).toBe(true);
  expect(l.sizes.reduce((a, b) => a + b, 0)).toBeCloseTo(1);
  const n = normalize({ ...bad, sizes: [0, 0, 0] }) as Extract<Layout, { kind: "split" }>;
  expect(n.sizes.every(Number.isFinite)).toBe(true);
});

test("split returns null for an unknown group", () => {
  expect(split(g("g1"), "nope", "row", "g2")).toBeNull();
});

test("removeGroup with an unknown id returns the tree unchanged", () => {
  const two = split(g("g1"), "g1", "row", "g2")!;
  expect(removeGroup(two, "nope")).toEqual(two);
});

test("splitting in the same direction resets uneven sizes to equal", () => {
  const two = resize(split(g("g1"), "g1", "row", "g2")!, [], [1, 3]);
  const three = split(two, "g1", "row", "g3") as Extract<Layout, { kind: "split" }>;
  for (const s of three.sizes) expect(s).toBeCloseTo(1 / 3);
});
