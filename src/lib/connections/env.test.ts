import { applyEnvironment, initialEnvState, ENVIRONMENTS, type EnvState } from "./env";

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

describe("environment defaults", () => {
  const custom = {
    production: { color: "#aa0000", guard: "block" as const },
    staging: { color: "#bb0000", guard: "confirm" as const },
    test: { color: "#cc0000", guard: "off" as const },
    local: { color: "#dd0000", guard: "off" as const },
  };
  it("environment defaults come from preferences", () => {
    const s = applyEnvironment(base, "production", custom);
    expect([s.color, s.guard]).toEqual(["#aa0000", "block"]);
    const fresh = initialEnvState(undefined, custom);
    expect([fresh.environment, fresh.color, fresh.colorTouched]).toEqual(["local", "#dd0000", false]);
  });
  it("a stored connection matching the preferences is untouched, one matching the factory is not", () => {
    const s = initialEnvState({ environment: "production", color: "#aa0000", guard: "block" }, custom);
    expect([s.colorTouched, s.guardTouched]).toEqual([false, false]);
    const f = initialEnvState({ environment: "production", color: "#e5566f", guard: "confirm" }, custom);
    expect([f.colorTouched, f.guardTouched]).toEqual([true, true]);
  });
});

describe("initialEnvState", () => {
  it("follows env changes when stored values are the defaults", () => {
    const s = applyEnvironment(initialEnvState({ environment: "local", color: "#4a8ad6", guard: "off" }), "production");
    expect([s.color, s.guard]).toEqual(["#e5566f", "confirm"]);
  });
  it("keeps hand-edited values", () => {
    const s = applyEnvironment(initialEnvState({ environment: "local", color: "#123456", guard: "block" }), "production");
    expect([s.color, s.guard]).toEqual(["#123456", "block"]);
  });
});
