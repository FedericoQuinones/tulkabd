import { expect, test } from "vitest";
import { defaultSaveName } from "./saveName";

test("the default name is result-yyyyMMdd-HHmmss with the format's extension", () => {
  const at = new Date(2026, 0, 5, 3, 4, 9);
  expect(defaultSaveName("csv", at)).toBe("result-20260105-030409.csv");
  expect(defaultSaveName("tsv", at)).toBe("result-20260105-030409.tsv");
  expect(defaultSaveName("json", new Date(2026, 10, 6, 15, 30, 42))).toBe("result-20261106-153042.json");
});
