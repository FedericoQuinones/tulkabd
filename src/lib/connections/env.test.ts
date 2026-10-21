import { applyEnvironment, ENVIRONMENTS, type EnvState } from "./env";

const base: EnvState = { environment: "local", color: "#4a8ad6", guard: "off", colorTouched: false, guardTouched: false };

describe("applyEnvironment", () => {
  it("updates color and guard when untouched", () => {
    const s = applyEnvironment(base, "production");
    expect([s.environment, s.color, s.guard]).toEqual(["production", "#e5566f", "confirm"]);
  });
  it("keeps color/guard the user changed by hand", () => {
    const s = applyEnvironment({ ...base, color: "#123456", colorTouched: true, guardTouched: true, guard: "block" }, "staging");
    expect([s.color, s.guard]).toEqual(["#123456", "block"]);
  });
  it("matches the Rust defaults", () => {
    expect(ENVIRONMENTS.map((e) => e.color)).toEqual(["#e5566f", "#d9a441", "#44a282", "#4a8ad6"]);
  });
});
