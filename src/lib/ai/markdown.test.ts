import { expect, test } from "vitest";
import { parseMarkdown } from "./markdown";

test("fenced_sql_block", () => {
  expect(parseMarkdown("Run:\n```sql\nSELECT 1;\nSELECT 2;\n```\nDone")).toEqual([
    { kind: "p", inline: [{ kind: "text", text: "Run:" }] },
    { kind: "code", lang: "sql", text: "SELECT 1;\nSELECT 2;" },
    { kind: "p", inline: [{ kind: "text", text: "Done" }] },
  ]);
});

test("bullets_and_numbers", () => {
  expect(parseMarkdown("- a\n- b\n\n1. x\n2. y")).toEqual([
    { kind: "list", ordered: false, items: [[{ kind: "text", text: "a" }], [{ kind: "text", text: "b" }]] },
    { kind: "list", ordered: true, items: [[{ kind: "text", text: "x" }], [{ kind: "text", text: "y" }]] },
  ]);
});

test("inline_code_and_bold", () => {
  expect(parseMarkdown("use `dbo.T` and **bold** and *it*")).toEqual([
    {
      kind: "p",
      inline: [
        { kind: "text", text: "use " },
        { kind: "code", text: "dbo.T" },
        { kind: "text", text: " and " },
        { kind: "bold", text: "bold" },
        { kind: "text", text: " and " },
        { kind: "italic", text: "it" },
      ],
    },
  ]);
});

test("html_stays_text", () => {
  const out = parseMarkdown("<img onerror=x>");
  expect(out).toEqual([{ kind: "p", inline: [{ kind: "text", text: "<img onerror=x>" }] }]);
});

test("headings_and_unclosed_fence", () => {
  expect(parseMarkdown("## Title\n```\nabc")).toEqual([
    { kind: "heading", level: 2, inline: [{ kind: "text", text: "Title" }] },
    { kind: "code", lang: "", text: "abc" },
  ]);
});

test("fence_line_with_many_spaces_is_fast", () => {
  const start = Date.now();
  parseMarkdown("```" + " ".repeat(10000) + "!");
  expect(Date.now() - start).toBeLessThan(500);
});
