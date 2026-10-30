#!/usr/bin/env python3
"""End-to-end smoke test of `tulkabd mcp` against the Docker SQL Server.

Usage (after scripts/mcp-seed.sql has run):
    python3 -I scripts/mcp_smoke.py --exe target/release/tulkabd \
        --visible "MCP Local" --hidden "MCP Prod"

The test never touches the real profile: it builds a throwaway HOME / XDG
tree with a config.json (visible connection: Local, read only; hidden one:
Production, no access) and a metadata cache of the seeded objects. The SQL
password of the visible connection is stored in the Secret Service under a
random connection id and always removed at the end. Without `secret-tool` or a
Secret Service session, the steps that need the password are skipped.
"""
import argparse
import json
import os
import shutil
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
import uuid

SERVER, PORT, USER, PASSWORD = "127.0.0.1", 14333, "sa", "Tulka_Test_2026!"
DATABASE = "tulka_mcp"
STEP_TIMEOUT = 30
results = []


def check(name, ok, detail=""):
    results.append(ok)
    print(("PASS" if ok else "FAIL") + "  " + name + (("  (" + detail + ")") if detail and not ok else ""))


def skip(name, why):
    print("SKIP  " + name + "  (" + why + ")")


def connection(cid, name, env, ai, password_saved):
    return {
        "id": cid, "name": name, "driver": "mssql", "server": SERVER, "port": PORT,
        "user": USER, "databases": {"kind": "selected", "names": [DATABASE]},
        "environment": env, "color": "#4a8ad6", "guard": "off", "encrypt": True,
        "trustServerCertificate": True, "connectTimeoutS": 15,
        "passwordSaved": password_saved, "folderId": None, "aiAccess": ai,
    }


def secret_service_ok():
    if not shutil.which("secret-tool"):
        return False
    probe = subprocess.run(["secret-tool", "lookup", "service", "tulkabd-smoke-probe-" + uuid.uuid4().hex],
                           capture_output=True, timeout=20)
    return probe.returncode in (0, 1) and not probe.stderr


def secret_attrs(cid):
    return ["service", "tulkabd", "username", cid, "target", "default", "application", "rust-keyring"]


def seed_cache(path, cid):
    db = sqlite3.connect(path)
    db.executescript("""
        CREATE TABLE databases (connection_id TEXT NOT NULL, name TEXT NOT NULL,
            no_access INTEGER NOT NULL DEFAULT 0, indexed_at INTEGER, PRIMARY KEY (connection_id, name));
        CREATE TABLE objects (id INTEGER PRIMARY KEY, connection_id TEXT NOT NULL, database TEXT NOT NULL,
            schema TEXT NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL);
        CREATE INDEX objects_by_database ON objects (connection_id, database);
        CREATE TABLE columns (object_id INTEGER NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL, name TEXT NOT NULL, type_name TEXT NOT NULL, nullable INTEGER NOT NULL,
            primary_key INTEGER NOT NULL, default_value TEXT, PRIMARY KEY (object_id, ordinal));
        PRAGMA user_version = 1;
    """)
    db.execute("INSERT INTO databases VALUES (?, ?, 0, ?)", (cid, DATABASE, int(time.time())))
    for i, (name, kind) in enumerate([("clientes", "table"), ("pedidos", "table"), ("v_pedidos_cliente", "view")], 1):
        db.execute("INSERT INTO objects VALUES (?, ?, ?, 'dbo', ?, ?)", (i, cid, DATABASE, name, kind))
    db.commit()
    db.close()


class Client:
    def __init__(self, exe, env):
        self.proc = subprocess.Popen([exe, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.DEVNULL, env=env, text=True, bufsize=1)
        self.n = 0

    def send(self, msg):
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def call(self, method, params=None):
        self.n += 1
        self.send({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params or {}})
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("server closed stdout")
            msg = json.loads(line)
            if msg.get("id") == self.n:
                return msg

    def tool(self, name, **args):
        reply = self.call("tools/call", {"name": name, "arguments": args})
        if "error" in reply:
            return True, json.dumps(reply["error"])
        res = reply["result"]
        text = "\n".join(c.get("text", "") for c in res.get("content", []))
        return bool(res.get("isError")), text

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()


def fresh_env(base, extra):
    env = {k: v for k, v in os.environ.items() if k not in ("DISPLAY", "WAYLAND_DISPLAY")}
    env.update(HOME=base, XDG_CONFIG_HOME=base + "/c", XDG_DATA_HOME=base + "/d",
               XDG_CACHE_HOME=base + "/k", XDG_STATE_HOME=base + "/s")
    env.update(extra)
    return env


def initialize(exe, env):
    t0 = time.perf_counter()
    c = Client(exe, env)
    reply = c.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                  "clientInfo": {"name": "mcp-smoke", "version": "1"}})
    ms = (time.perf_counter() - t0) * 1000
    return c, reply, ms


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--visible", default="MCP Local")
    ap.add_argument("--hidden", default="MCP Prod")
    a = ap.parse_args()
    exe = os.path.abspath(a.exe)

    base = tempfile.mkdtemp(prefix="tulkabd-smoke-")
    cid, hid = str(uuid.uuid4()), str(uuid.uuid4())
    stored = False
    try:
        keyring = secret_service_ok()
        if keyring:
            r = subprocess.run(["secret-tool", "store", "--label=tulkabd smoke (temporary)"] + secret_attrs(cid),
                               input=PASSWORD, text=True, capture_output=True, timeout=30)
            stored = r.returncode == 0
        if not stored:
            print("NOTE  no usable Secret Service: steps that log in to SQL Server are skipped")
        cfg_dir, data_dir = base + "/c/tulkabd", base + "/d/tulkabd"
        os.makedirs(cfg_dir)
        os.makedirs(data_dir)
        with open(cfg_dir + "/config.json", "w") as f:
            json.dump({"version": 1, "connections": [
                connection(cid, a.visible, "local", "readOnly", stored),
                connection(hid, a.hidden, "production", "none", False)]}, f)
        seed_cache(data_dir + "/tulkabd.db", cid)
        env = fresh_env(base, {})
        # secret-tool needs the real session bus: kept from os.environ above.

        # 1. initialize and tools
        c, reply, first_ms = initialize(exe, env)
        c.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        tools = sorted(t["name"] for t in c.call("tools/list")["result"]["tools"])
        want = ["describe_table", "list_connections", "list_databases", "run_query", "search_objects"]
        check("1 initialize + tools " + ",".join(tools), "result" in reply and tools == want)

        # 2. list_connections
        err, text = c.tool("list_connections")
        check("2 list_connections shows visible, not hidden",
              not err and a.visible in text and a.hidden not in text, text[:200])

        # 3. search_objects
        err, text = c.tool("search_objects", query="pedido")
        check("3 search_objects finds dbo.pedidos and dbo.v_pedidos_cliente",
              not err and "pedidos" in text and "v_pedidos_cliente" in text, text[:300])

        if stored:
            # 4. describe_table
            err, text = c.tool("describe_table", connection=a.visible, table="pedidos")
            low = text.lower()
            check("4 describe_table: FK to clientes and index",
                  not err and "clientes" in low and "ix_pedidos_creado" in low, text[:400])

            # 5. TOP 3
            err, text = c.tool("run_query", connection=a.visible,
                               sql="SELECT TOP 3 id, total FROM dbo.pedidos ORDER BY id")
            check("5 run_query TOP 3 gives 3 rows",
                  not err and "3 rows" in text.lower() or ("3 row" in text.lower() and not err), text[:300])

            # 6. 500 rows + limit
            err, text = c.tool("run_query", connection=a.visible, sql="SELECT * FROM dbo.pedidos")
            check("6 run_query gives 500 rows and 'limit reached'",
                  not err and "500" in text and "limit reached" in text.lower(), text[:100] + " ... " + text[-200:])

            # 7. refused statements
            for sql in ["DELETE FROM dbo.pedidos", "SELECT * INTO x FROM dbo.pedidos",
                        "SELECT NEXT VALUE FOR dbo.seq"]:
                err, text = c.tool("run_query", connection=a.visible, sql=sql)
                check("7 refused: " + sql, err and "Not run" in text, text[:200])

            # 8. nothing changed
            err, text = c.tool("run_query", connection=a.visible, sql="SELECT COUNT(*) AS n FROM dbo.pedidos")
            check("8 COUNT(*) still 5000", not err and "5000" in text, text[:200])
        else:
            for n in "45678":
                skip("step " + n, "needs the Secret Service")

        # 9. hidden connection
        err, text = c.tool("run_query", connection=a.hidden, sql="SELECT 1")
        check("9 hidden connection: 'No connection named'", err and "No connection named" in text, text[:200])
        c.close()

        # 10. cold initialize latency (median of 10, no display)
        times = []
        for _ in range(10):
            c2, r2, ms = initialize(exe, env)
            times.append(ms)
            c2.close()
        med = statistics.median(times)
        check("10 cold initialize median %.0f ms (budget 300) over %s" % (med, [round(t) for t in times]), med < 300)
    except Exception as e:  # report, then fall through to cleanup
        check("unexpected error: " + type(e).__name__, False, str(e))
    finally:
        if stored:
            subprocess.run(["secret-tool", "clear"] + secret_attrs(cid), capture_output=True, timeout=30)
            left = subprocess.run(["secret-tool", "lookup"] + secret_attrs(cid), capture_output=True, timeout=30)
            print("NOTE  temporary secret " + ("STILL PRESENT" if left.stdout else "removed"))
        shutil.rmtree(base, ignore_errors=True)
    sys.exit(0 if results and all(results) else 1)


if __name__ == "__main__":
    main()
