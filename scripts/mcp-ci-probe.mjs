// Speaks MCP over stdio to the release binary, the way an AI client does:
// initialize -> tools/list -> close stdin -> expect exit 0.
// Usage: node scripts/mcp-ci-probe.mjs <path-to-tulkabd>
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const EXPECTED = ["list_connections", "list_databases", "search_objects", "describe_table", "run_query"];
const STEP_MS = 10_000;
const EXIT_MS = 5_000;

const exe = process.argv[2];
if (!exe) {
  console.error("usage: node scripts/mcp-ci-probe.mjs <path-to-tulkabd>");
  process.exit(2);
}

// Temporary profile dirs: the probe must never read or write a real config.
const home = mkdtempSync(join(tmpdir(), "tulkabd-probe-"));
const env = {
  ...process.env,
  HOME: home,
  USERPROFILE: home,
  APPDATA: join(home, "AppData", "Roaming"),
  LOCALAPPDATA: join(home, "AppData", "Local"),
  XDG_CONFIG_HOME: join(home, ".config"),
  XDG_DATA_HOME: join(home, ".local", "share"),
  XDG_CACHE_HOME: join(home, ".cache"),
  XDG_STATE_HOME: join(home, ".local", "state"),
};

const child = spawn(exe, ["mcp"], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true, env });

let stderr = "";
child.stderr.on("data", (d) => (stderr += d));
let exited = null;
const exitPromise = new Promise((resolve) => {
  child.on("close", (code, signal) => {
    exited = { code, signal };
    resolve(exited);
  });
  child.on("error", (error) => {
    exited = { code: null, signal: null, error };
    resolve(exited);
  });
});
// A write to a dead child must not crash the probe with EPIPE.
child.stdin.on("error", () => {});

function cleanup() {
  try {
    rmSync(home, { recursive: true, force: true });
  } catch {}
}

function fail(msg) {
  console.error(`MCP probe FAILED: ${msg}`);
  if (stderr.trim()) console.error(`--- child stderr ---\n${stderr.trim()}`);
  if (!exited) child.kill();
  cleanup();
  process.exit(1);
}

const waiters = new Map();
const seen = new Map();
let buf = "";
child.stdout.on("data", (d) => {
  buf += d;
  let i;
  while ((i = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, i).trim();
    buf = buf.slice(i + 1);
    if (!line) continue;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      fail(`stdout line is not JSON: ${line.slice(0, 200)}`);
    }
    if (msg.id === undefined) continue;
    const w = waiters.get(msg.id);
    if (w) {
      waiters.delete(msg.id);
      w(msg);
    } else seen.set(msg.id, msg);
  }
});

function send(obj) {
  child.stdin.write(JSON.stringify(obj) + "\n");
}

function request(id, method, params) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`no reply to ${method} within ${STEP_MS / 1000}s`)),
      STEP_MS,
    );
    waiters.set(id, (m) => {
      clearTimeout(timer);
      resolve(m);
    });
    send({ jsonrpc: "2.0", id, method, params });
  });
}

const earlyExit = exitPromise.then((r) => {
  throw new Error(
    r.error
      ? `could not run ${exe}: ${r.error.message}`
      : `process exited early (code ${r.code}, signal ${r.signal})`,
  );
});

try {
  const init = await Promise.race([
    request(1, "initialize", {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "tulkabd-ci-probe", version: "0" },
    }),
    earlyExit,
  ]);
  if (!init.result?.protocolVersion) fail(`initialize reply has no protocolVersion: ${JSON.stringify(init)}`);

  send({ jsonrpc: "2.0", method: "notifications/initialized" });

  const list = await Promise.race([request(2, "tools/list", {}), earlyExit]);
  const names = (list.result?.tools ?? []).map((t) => t.name);
  const missing = EXPECTED.filter((n) => !names.includes(n));
  if (missing.length) fail(`tools/list is missing ${missing.join(", ")} (got: ${names.join(", ") || "none"})`);

  child.stdin.end();
  const result = await Promise.race([
    exitPromise,
    new Promise((r) => setTimeout(() => r(null), EXIT_MS)),
  ]);
  if (!result) fail(`process did not exit within ${EXIT_MS / 1000}s after stdin closed`);
  if (result.code !== 0) fail(`process exited with code ${result.code} (signal ${result.signal}), expected 0`);
} catch (e) {
  fail(e.message);
}

cleanup();
console.log(`MCP probe OK: ${EXPECTED.length} tools, clean exit`);
