import { isNamedInstance, validateConnection, type ConnectionForm } from "./validate";

const valid: ConnectionForm = { name: "Docker local", server: "127.0.0.1", port: 14333, timeout: 15 };

describe("validateConnection", () => {
  it("accepts a valid form", () => {
    expect(validateConnection(valid)).toEqual({});
  });
  it("requires a name", () => {
    expect(validateConnection({ ...valid, name: "  " })).toEqual({
      name: "connection.errors.nameRequired",
    });
  });
  it("rejects ports outside 1-65535", () => {
    expect(validateConnection({ ...valid, port: 0 })).toEqual({ port: "connection.errors.portRange" });
    expect(validateConnection({ ...valid, port: 70000 })).toEqual({ port: "connection.errors.portRange" });
    expect(validateConnection({ ...valid, port: 1.5 })).toEqual({ port: "connection.errors.portRange" });
  });
  it("requires a server", () => {
    expect(validateConnection({ ...valid, server: "" })).toEqual({
      server: "connection.errors.serverRequired",
    });
  });
  it("accepts host\\instance and rejects malformed instance names", () => {
    expect(validateConnection({ ...valid, server: "db01\\SQLEXPRESS" })).toEqual({});
    for (const server of ["db01\\", "\\SQLEXPRESS", "db01\\a\\b", " \\ "]) {
      expect(validateConnection({ ...valid, server })).toEqual({
        server: "connection.errors.serverInstance",
      });
    }
  });
  it("tells named instances apart", () => {
    expect(isNamedInstance("db01\\SQLEXPRESS")).toBe(true);
    expect(isNamedInstance("db01")).toBe(false);
  });
  it("requires a connect timeout of 1-600 whole seconds", () => {
    for (const timeout of [null, 0, -1, 601, 2.5]) {
      expect(validateConnection({ ...valid, timeout })).toEqual({
        timeout: "connection.errors.timeoutRange",
      });
    }
    expect(validateConnection({ ...valid, timeout: 1 })).toEqual({});
    expect(validateConnection({ ...valid, timeout: 600 })).toEqual({});
  });
});
