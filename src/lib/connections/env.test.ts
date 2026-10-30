import { applyEnvironment, initialEnvState, ENVIRONMENTS, type EnvState } from "./env";

const base: EnvState = { environment: "local", color: "#4a8ad6", guard: "off", aiAccess: "readOnly", colorTouched: false, guardTouched: false, aiTouched: false };

describe("applyEnvironment", () => {
  it("updates color and guard when untouched", () => {
    const s = applyEnvironment(base, "production");
    expect([s.environment, s.color, s.guard]).toEqual(["production", "#e5566f", "confirm"]);
  });
  it("keeps color/guard the user changed by hand", () => {
    const s = applyEnvironment({ ...base, color: "#123456", colorTouched: true, guardTouched: true, guard: "block", aiAccess: "none" }, "staging");
    expect([s.color, s.guard]).toEqual(["#123456", "block"]);
  });
  it("matches the Rust defaults", () => {
    expect(ENVIRONMENTS.map((e) => e.color)).toEqual(["#e5566f", "#d9a441", "#44a282", "#4a8ad6"]);
  });
});

describe("environment defaults", () => {
  const custom = {
    production: { color: "#aa0000", guard: "block" as const, aiAccess: "readOnly" as const },
    staging: { color: "#bb0000", guard: "confirm" as const, aiAccess: "none" as const },
    test: { color: "#cc0000", guard: "off" as const, aiAccess: "none" as const },
    local: { color: "#dd0000", guard: "off" as const, aiAccess: "none" as const },
  };
  it("environment defaults come from preferences", () => {
    const s = applyEnvironment(base, "production", custom);
    expect([s.color, s.guard]).toEqual(["#aa0000", "block"]);
    const fresh = initialEnvState(undefined, custom);
    expect([fresh.environment, fresh.color, fresh.colorTouched]).toEqual(["local", "#dd0000", false]);
  });
  it("a stored connection matching the preferences is untouched, one matching the factory is not", () => {
    const s = initialEnvState({ environment: "production", color: "#aa0000", guard: "block", aiAccess: "none" }, custom);
    expect([s.colorTouched, s.guardTouched]).toEqual([false, false]);
    const f = initialEnvState({ environment: "production", color: "#e5566f", guard: "confirm", aiAccess: "none" }, custom);
    expect([f.colorTouched, f.guardTouched]).toEqual([true, true]);
  });
});

describe("initialEnvState", () => {
  it("follows env changes when stored values are the defaults", () => {
    const s = applyEnvironment(initialEnvState({ environment: "local", color: "#4a8ad6", guard: "off", aiAccess: "readOnly" }), "production");
    expect([s.color, s.guard]).toEqual(["#e5566f", "confirm"]);
  });
  it("keeps hand-edited values", () => {
    const s = applyEnvironment(initialEnvState({ environment: "local", color: "#123456", guard: "block", aiAccess: "none" }), "production");
    expect([s.color, s.guard]).toEqual(["#123456", "block"]);
  });
});

describe("AI access", () => {
  it("production starts with no AI access", () => {
    expect(ENVIRONMENTS.map((e) => e.aiAccess)).toEqual(["none", "readOnly", "readOnly", "readOnly"]);
    expect(applyEnvironment(base, "production").aiAccess).toBe("none");
  });
  it("switching environment moves untouched AI access", () => {
    const s = applyEnvironment(applyEnvironment(base, "production"), "staging");
    expect(s.aiAccess).toBe("readOnly");
  });
  it("touched AI access stays", () => {
    const s = applyEnvironment({ ...base, aiAccess: "none", aiTouched: true }, "staging");
    expect(s.aiAccess).toBe("none");
  });
  it("stored value different from the default marks it touched", () => {
    const s = initialEnvState({ environment: "local", color: "#4a8ad6", guard: "off", aiAccess: "none" });
    expect([s.aiAccess, s.aiTouched, s.guardTouched]).toEqual(["none", true, false]);
    expect(initialEnvState({ environment: "local", color: "#4a8ad6", guard: "off", aiAccess: "readOnly" }).aiTouched).toBe(false);
  });
});
