// Pure layout tree for the split workspace: leaves are editor groups, inner nodes are splits.
export type Layout =
  | { kind: "group"; id: string }
  | { kind: "split"; dir: "row" | "column"; children: Layout[]; sizes: number[] }; // sizes sum to 1

type SplitNode = Extract<Layout, { kind: "split" }>;

export const MAX_GROUPS = 4;

/** Group ids in reading order. */
export function groups(l: Layout): string[] {
  return l.kind === "group" ? [l.id] : l.children.flatMap(groups);
}

function equal(n: number): number[] {
  return Array.from({ length: n }, () => 1 / n);
}

/** Rescales to sum 1; falls back to equal sizes when the total is not a finite number above 0. */
function scale(sizes: number[]): number[] {
  const total = sizes.reduce((a, b) => a + b, 0);
  if (!Number.isFinite(total) || !(total > 0)) return equal(sizes.length);
  return sizes.map((s) => s / total);
}

function splitIn(l: Layout, groupId: string, dir: "row" | "column", newId: string): Layout {
  if (l.kind === "group") {
    if (l.id !== groupId) return l;
    return {
      kind: "split",
      dir,
      children: [l, { kind: "group", id: newId }],
      sizes: [0.5, 0.5],
    };
  }
  const at = l.children.findIndex((c) => c.kind === "group" && c.id === groupId);
  if (at >= 0 && l.dir === dir) {
    const children = [...l.children];
    children.splice(at + 1, 0, { kind: "group", id: newId });
    return { ...l, children, sizes: equal(children.length) };
  }
  return { ...l, children: l.children.map((c) => splitIn(c, groupId, dir, newId)) };
}

/** Splits a group in two; null when the workspace already has MAX_GROUPS groups or the group is not in it. */
export function split(
  l: Layout,
  groupId: string,
  dir: "row" | "column",
  newId: string,
): Layout | null {
  const ids = groups(l);
  if (ids.length >= MAX_GROUPS || !ids.includes(groupId)) return null;
  return normalize(splitIn(l, groupId, dir, newId));
}

function removeIn(l: Layout, groupId: string): Layout | null {
  if (l.kind === "group") return l.id === groupId ? null : l;
  const children: Layout[] = [];
  const sizes: number[] = [];
  l.children.forEach((c, i) => {
    const kept = removeIn(c, groupId);
    if (kept) {
      children.push(kept);
      sizes.push(l.sizes[i]);
    }
  });
  if (children.length === 0) return null;
  return { ...l, children, sizes: scale(sizes) };
}

/** Removes a group; the last remaining group is never removed. */
export function removeGroup(l: Layout, groupId: string): Layout {
  if (groups(l).length <= 1) return l;
  return normalize(removeIn(l, groupId) ?? l);
}

/** Sets the sizes of the split at `path` (child indexes from the root); sizes are rescaled to sum 1. */
export function resize(l: Layout, path: number[], sizes: number[]): Layout {
  if (l.kind === "group") return l;
  if (path.length === 0) {
    if (sizes.length !== l.children.length || sizes.some((s) => !Number.isFinite(s) || !(s > 0))) {
      return l;
    }
    return { ...l, sizes: scale(sizes) };
  }
  const [head, ...rest] = path;
  if (head < 0 || head >= l.children.length) return l;
  const children = l.children.map((c, i) => (i === head ? resize(c, rest, sizes) : c));
  return { ...l, children };
}

/** Collapses single-child splits and splits nested in a parent of the same direction. */
export function normalize(l: Layout): Layout {
  if (l.kind === "group") return l;
  const children: Layout[] = [];
  const sizes: number[] = [];
  l.children.forEach((raw, i) => {
    const c = normalize(raw);
    if (c.kind === "split" && c.dir === l.dir) {
      c.children.forEach((cc, j) => {
        children.push(cc);
        sizes.push(l.sizes[i] * c.sizes[j]);
      });
    } else {
      children.push(c);
      sizes.push(l.sizes[i]);
    }
  });
  if (children.length === 1) return children[0];
  const node: SplitNode = { ...l, children, sizes: scale(sizes) };
  return node;
}
