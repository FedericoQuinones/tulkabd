import { expect, test } from "vitest";
import { weakerTlsDropsPassword } from "./savedPassword";

const stored = { environment: "production" as const, encrypt: true, trustServerCertificate: false, passwordSaved: true };
const same = { environment: "production" as const, encrypt: true, trustServerCertificate: false };

test("no warning while TLS is not weaker", () => {
  expect(weakerTlsDropsPassword(stored, same, "")).toBe(false);
});

test("turning encryption off or trusting the certificate warns", () => {
  expect(weakerTlsDropsPassword(stored, { ...same, encrypt: false }, "")).toBe(true);
  expect(weakerTlsDropsPassword(stored, { ...same, trustServerCertificate: true }, "")).toBe(true);
});

test("already weak stored settings are not weaker", () => {
  const weak = { ...stored, encrypt: false, trustServerCertificate: true };
  expect(weakerTlsDropsPassword(weak, { ...same, encrypt: false, trustServerCertificate: true }, "")).toBe(false);
});

test("a typed password, no saved password or a new connection never warns", () => {
  expect(weakerTlsDropsPassword(stored, { ...same, encrypt: false }, "x")).toBe(false);
  expect(weakerTlsDropsPassword({ ...stored, passwordSaved: false }, { ...same, encrypt: false }, "")).toBe(false);
  expect(weakerTlsDropsPassword(undefined, { ...same, encrypt: false }, "")).toBe(false);
});

test("Local to Local never warns", () => {
  const local = { ...stored, environment: "local" as const };
  expect(weakerTlsDropsPassword(local, { ...same, environment: "local", trustServerCertificate: true }, "")).toBe(false);
});

test("changing the environment to or from Local still warns", () => {
  expect(weakerTlsDropsPassword(stored, { ...same, environment: "local", trustServerCertificate: true }, "")).toBe(true);
  const local = { ...stored, environment: "local" as const };
  expect(weakerTlsDropsPassword(local, { ...same, environment: "test", encrypt: false }, "")).toBe(true);
});
