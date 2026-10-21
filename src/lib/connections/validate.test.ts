import { validateConnection, type ConnectionForm } from "./validate";

const valid: ConnectionForm = { name: "Docker local", server: "127.0.0.1", port: 14333 };

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
});
