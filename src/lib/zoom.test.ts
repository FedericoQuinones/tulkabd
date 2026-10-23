import { expect, test } from "vitest";
import { stepZoom } from "./zoom";

test("zoom moves in 10 % steps", () => {
  expect(stepZoom(100, "zoomIn")).toBe(110);
  expect(stepZoom(100, "zoomOut")).toBe(90);
  expect(stepZoom(130, "zoomReset")).toBe(100);
});
test("five zoom-ins from 100 reach 150", () => {
  let z = 100;
  for (let i = 0; i < 5; i++) z = stepZoom(z, "zoomIn");
  expect(z).toBe(150);
});
test("zoom stops at 70 and 200", () => {
  expect(stepZoom(200, "zoomIn")).toBe(200);
  expect(stepZoom(70, "zoomOut")).toBe(70);
  expect(stepZoom(195, "zoomIn")).toBe(200);
});
