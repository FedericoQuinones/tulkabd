export type Inline = { kind: "text" | "code" | "bold" | "italic"; text: string };
export type Block =
  | { kind: "p"; inline: Inline[] }
  | { kind: "list"; ordered: boolean; items: Inline[][] }
  | { kind: "code"; lang: string; text: string }
  | { kind: "heading"; level: 1 | 2 | 3; inline: Inline[] };

const INLINE = /`([^`]+)`|\*\*([^*]+)\*\*|\*([^*\s][^*]*)\*/g;

function parseInline(src: string): Inline[] {
  const out: Inline[] = [];
  let last = 0;
  for (const m of src.matchAll(INLINE)) {
    if (m.index > last) out.push({ kind: "text", text: src.slice(last, m.index) });
    if (m[1] !== undefined) out.push({ kind: "code", text: m[1] });
    else if (m[2] !== undefined) out.push({ kind: "bold", text: m[2] });
    else out.push({ kind: "italic", text: m[3] });
    last = m.index + m[0].length;
  }
  if (last < src.length) out.push({ kind: "text", text: src.slice(last) });
  return out;
}

/**
 * Reads the small subset of markdown an answer uses into plain data. Nothing here produces HTML:
 * markup in the source stays text for the view to show as it is.
 */
export function parseMarkdown(src: string): Block[] {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let para: string[] = [];
  const flush = () => {
    if (para.length) blocks.push({ kind: "p", inline: parseInline(para.join(" ")) });
    para = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const fence = /^\s*```(?:\s*([\w+-]+))?\s*$/.exec(line);
    if (fence) {
      flush();
      const body: string[] = [];
      i++;
      while (i < lines.length && !/^\s*```\s*$/.test(lines[i])) body.push(lines[i++]);
      blocks.push({ kind: "code", lang: fence[1] ?? "", text: body.join("\n") });
      continue;
    }
    if (line.trim().startsWith("|")) {
      flush();
      const rows = [line];
      while (i + 1 < lines.length && lines[i + 1].trim().startsWith("|")) rows.push(lines[++i]);
      blocks.push({ kind: "code", lang: "", text: rows.join("\n") });
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      flush();
      blocks.push({ kind: "heading", level: Math.min(heading[1].length, 3) as 1 | 2 | 3, inline: parseInline(heading[2].trim()) });
      continue;
    }
    const item = /^\s*(?:([-*+])|(\d+)[.)])\s+(.*)$/.exec(line);
    if (item) {
      flush();
      const ordered = item[2] !== undefined;
      const prev = blocks[blocks.length - 1];
      const inl = parseInline(item[3]);
      if (prev && prev.kind === "list" && prev.ordered === ordered) prev.items.push(inl);
      else blocks.push({ kind: "list", ordered, items: [inl] });
      continue;
    }
    if (line.trim() === "") flush();
    else para.push(line.trim());
  }
  flush();
  return blocks;
}
