import { expect, test } from "vitest";
import { searchPhrases } from "./searchPhrases";
import en from "../i18n/en.json";

test("every search phrase resolves to a catalog string", () => {
  for (const [k, v] of Object.entries(searchPhrases())) {
    expect(v, k).not.toMatch(/^editor\.search\./);
  }
  expect(Object.keys(en).filter((k) => k.startsWith("editor.search.")).length).toBe(Object.keys(searchPhrases()).length);
});
