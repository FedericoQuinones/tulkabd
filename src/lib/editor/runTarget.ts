/**
 * Picks what Run executes: a non-blank selection, or the whole document.
 * `lineOffset` is the 0-based line where the SQL starts; backend error lines
 * (1-based, relative to the submitted SQL) are added to it to find the editor line.
 */
export function runTarget(doc: string, selFrom: number, selTo: number): { sql: string; lineOffset: number } {
  const from = Math.min(selFrom, selTo);
  const to = Math.max(selFrom, selTo);
  const selected = doc.slice(from, to);
  if (selected.trim() === "") return { sql: doc, lineOffset: 0 };
  let lineOffset = 0;
  for (let i = doc.indexOf("\n"); i !== -1 && i < from; i = doc.indexOf("\n", i + 1)) lineOffset++;
  return { sql: selected, lineOffset };
}
