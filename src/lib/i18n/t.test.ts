import { t } from "./t";
test("interpolates params", () => {
  expect(t("results.rowsShown", { shown: 12, total: 20 })).toBe("12 of 20 rows");
});
test("returns the key when missing", () => {
  expect(t("does.not.exist")).toBe("does.not.exist");
});
