import type { Value } from "../ipc";

export type CellKind = "null" | "number" | "text" | "binary";

const MAX_BYTES = 32;

/** Adds "," thousands separators to the integer part of a plain decimal string (no number round trip). */
export function groupDigits(s: string): string {
  const m = /^(-?)(\d+)(\.\d*)?$/.exec(s);
  if (!m) return s;
  return m[1] + m[2].replace(/\B(?=(\d{3})+(?!\d))/g, ",") + (m[3] ?? "");
}

/** Display text for a cell; the grouping is for display only (Rust formats what gets copied). */
export function cellText(v: Value): { text: string; kind: CellKind } {
  switch (v.kind) {
    case "null":
      return { text: "NULL", kind: "null" };
    case "int":
    case "decimal":
      return { text: groupDigits(v.value), kind: "number" };
    case "float":
      return { text: groupDigits(String(v.value)), kind: "number" };
    case "bytes": {
      const hex = v.value
        .slice(0, MAX_BYTES)
        .map((b) => b.toString(16).padStart(2, "0"))
        .join("")
        .toUpperCase();
      return { text: "0x" + hex + (v.value.length > MAX_BYTES ? "…" : ""), kind: "binary" };
    }
    case "bool":
      return { text: v.value ? "1" : "0", kind: "text" };
    default:
      return { text: v.value, kind: "text" };
  }
}
