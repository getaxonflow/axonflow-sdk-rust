#!/usr/bin/env bash
#
# Proves that a process which constructs a client, makes ONE call and exits
# still delivers its telemetry ping.
#
# WHY THIS EXISTS. The first-request heartbeat trigger (enterprise#3682) is
# there to make short-lived processes visible. If the send is SPAWNED, this
# exact shape loses it: the runtime is dropped at exit and the POST is
# cancelled. Measured at 1 delivery in 12 before the send was awaited inline on
# the cold path — while the /health GET reached the platform every time, so the
# SDK made an unsolicited request to someone else's server and recorded nothing.
#
# TWELVE RUNS, not one. A spawned send delivers occasionally, so a single run
# would pass against the defect roughly one time in twelve.
set -euo pipefail
cd "$(dirname "$0")"

RUNS="${RUNS:-12}"
PORT_FILE=$(mktemp)
LOG=$(mktemp)

# A listener that records /health hits and ping POSTs separately. The PATH is
# checked, not just the method: the driver's own API call must not be counted
# as a /health fetch.
python3 - "$PORT_FILE" "$LOG" <<'PY' &
import http.server, socketserver, sys, threading, json
port_file, log = sys.argv[1], sys.argv[2]
counts = {"health": 0, "ping": 0}
class H(http.server.BaseHTTPRequestHandler):
    def _write(self, code, body=b"{}"):
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        if self.path.startswith("/health"):
            counts["health"] += 1
            self._write(200, b'{"status":"healthy","version":"10.4.0","tier":"Community"}')
        else:
            self._write(200, b'{"connectors":[]}')
    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(length)
        counts["ping"] += 1
        open(log, "w").write(json.dumps(counts))
        self._write(200, b'{"latest_version":null}')
    def log_message(self, *a): pass
class S(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
srv = S(("127.0.0.1", 0), H)
open(port_file, "w").write(str(srv.server_address[1]))
open(log, "w").write(json.dumps(counts))
srv.serve_forever()
PY
LISTENER=$!
trap 'kill $LISTENER 2>/dev/null || true' EXIT
sleep 1
PORT=$(cat "$PORT_FILE")
BASE="http://127.0.0.1:${PORT}"

cargo build --quiet --manifest-path helper/Cargo.toml
BIN="helper/target/debug/fast-exit-delivery"

# A private cache dir per invocation: the 7-day stamp would otherwise suppress
# every run after the first, and the test would pass vacuously.
delivered=0
for i in $(seq 1 "$RUNS"); do
  TMPHOME=$(mktemp -d)
  env -u AXONFLOW_TELEMETRY \
      AXONFLOW_CHECKPOINT_URL="${BASE}/v1/ping" \
      HOME="$TMPHOME" XDG_CACHE_HOME="$TMPHOME/.cache" \
      "$BIN" "$BASE" >/dev/null 2>&1 || true
  rm -rf "$TMPHOME"
done
sleep 1
PINGS=$(python3 -c "import json,sys;print(json.load(open('$LOG'))['ping'])")
HEALTH=$(python3 -c "import json,sys;print(json.load(open('$LOG'))['health'])")

echo "runs=$RUNS  pings=$PINGS  health_fetches=$HEALTH"
if [ "$PINGS" -ne "$RUNS" ]; then
  echo "FAIL: $PINGS/$RUNS pings delivered."
  echo "  A spawned send loses the race when the process exits; the cold path"
  echo "  must be awaited inline. Note health_fetches=$HEALTH — an undelivered"
  echo "  run still contacted the platform, which is worse than silence."
  exit 1
fi
echo "PASS: $PINGS/$RUNS pings delivered from a process that exits immediately"
