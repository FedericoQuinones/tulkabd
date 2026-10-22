import { beforeEach, expect, test, vi } from "vitest";

vi.mock("../ipc", () => ({ parseConnectionString: vi.fn() }));

import * as ipc from "../ipc";
import type { ParsedConnection } from "../ipc";
import { connStrErrorText, fillFrom, ignoredText, MAX_PASTE, parsePasted, proposeName } from "./connstr";

const parsed = (over: Partial<ParsedConnection> = {}): ParsedConnection => ({
  server: "db.empresa.com",
  port: 1444,
  user: "fede",
  password: "a;b",
  database: "Ventas",
  encrypt: null,
  trustServerCertificate: true,
  connectTimeoutS: null,
  ignoredKeys: [],
  unknownKeys: 0,
  ...over,
});

beforeEach(() => vi.resetAllMocks());

test("the proposed name is '<database> @ <server>', or the server", () => {
  expect(proposeName(parsed())).toBe("Ventas @ db.empresa.com");
  expect(proposeName(parsed({ database: null }))).toBe("db.empresa.com");
});

test("fill sets the server's fields and only the options the string names", () => {
  expect(fillFrom(parsed())).toEqual({
    server: "db.empresa.com",
    port: 1444,
    user: "fede",
    password: "a;b",
    databases: { kind: "selected", names: ["Ventas"] },
    trust: true,
  });
  expect(
    fillFrom(parsed({ port: null, user: null, password: null, database: null, encrypt: false, connectTimeoutS: 30 })),
  ).toEqual({
    server: "db.empresa.com",
    port: 1433,
    user: "",
    password: "",
    databases: { kind: "all" },
    encrypt: false,
    trust: true,
    timeout: 30,
  });
});

test("error codes become messages; bad_value names its key", () => {
  expect(connStrErrorText("unsupported_auth")).toMatch(/SQL Server login/);
  expect(connStrErrorText("bad_value:Connect Timeout")).toContain("Connect Timeout");
  expect(connStrErrorText("no_server")).not.toContain("connection.connstr");
  expect(connStrErrorText("something else")).toBe(connStrErrorText("unrecognized"));
});

test("a pasted text that does not parse yields null without reporting anything", async () => {
  vi.mocked(ipc.parseConnectionString).mockRejectedValue("unrecognized");
  expect(await parsePasted("hola mundo")).toBeNull();
  expect(ipc.parseConnectionString).toHaveBeenCalledWith("hola mundo");
});

test("empty or huge pastes are not sent to the backend", async () => {
  expect(await parsePasted("")).toBeNull();
  expect(await parsePasted("   ")).toBeNull();
  expect(await parsePasted(undefined)).toBeNull();
  expect(await parsePasted("x".repeat(MAX_PASTE + 1))).toBeNull();
  expect(ipc.parseConnectionString).not.toHaveBeenCalled();
});

test("a recognized paste yields the parsed connection", async () => {
  vi.mocked(ipc.parseConnectionString).mockResolvedValue(parsed());
  expect(await parsePasted("Server=db.empresa.com,1444")).toEqual(parsed());
});

test("the ignored note names well-known keys and only counts unknown ones", () => {
  expect(ignoredText(parsed())).toBeNull();
  expect(ignoredText(parsed({ ignoredKeys: ["Application Name"] }))).toBe("Ignored: Application Name");
  expect(ignoredText(parsed({ ignoredKeys: ["Application Name", "Pooling"], unknownKeys: 1 }))).toBe(
    "Ignored: Application Name, Pooling and 1 unknown setting",
  );
  expect(ignoredText(parsed({ unknownKeys: 3 }))).toBe("Ignored: 3 unknown settings");
});
