#!/usr/bin/env bash
# Starts (or reuses) a SQL Server 2022 container for integration tests and
# prints the TULKA_MSSQL_TEST export line to use with `cargo test`.
set -euo pipefail

NAME="tulka-mssql"
PORT=14333
PASSWORD='Tulka_Test_2026!'
IMAGE="mcr.microsoft.com/mssql/server:2022-latest"
TIMEOUT=60

if docker ps -a --format '{{.Names}}' | grep -qx "$NAME"; then
  if [ "$(docker inspect -f '{{.State.Running}}' "$NAME")" != "true" ]; then
    docker start "$NAME" >/dev/null
  fi
else
  docker run -d --name "$NAME" \
    -e ACCEPT_EULA=Y \
    -e MSSQL_SA_PASSWORD="$PASSWORD" \
    -p "$PORT:1433" \
    "$IMAGE" >/dev/null
fi

# Locate sqlcmd inside the container (path differs between image versions).
SQLCMD=""
for candidate in /opt/mssql-tools18/bin/sqlcmd /opt/mssql-tools/bin/sqlcmd; do
  if docker exec "$NAME" test -x "$candidate" 2>/dev/null; then
    SQLCMD="$candidate"
    break
  fi
done
if [ -z "$SQLCMD" ]; then
  echo "sqlcmd not found in container $NAME" >&2
  exit 1
fi

ready=0
for _ in $(seq 1 "$TIMEOUT"); do
  if docker exec "$NAME" "$SQLCMD" -C -S localhost -U sa -P "$PASSWORD" -Q "SELECT 1" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 1
done

if [ "$ready" -ne 1 ]; then
  echo "SQL Server did not become ready within ${TIMEOUT}s" >&2
  exit 1
fi

echo "export TULKA_MSSQL_TEST=\"server=127.0.0.1;port=$PORT;user=sa;password=$PASSWORD\""
