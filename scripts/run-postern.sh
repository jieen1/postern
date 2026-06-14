#!/usr/bin/env bash
# Launch Postern on Linux/WSL: the daemon (posternd) + the desktop console.
#
# Self-contained: finds posternd + the console app next to this script (a
# packaged dist/) or falls back to the repo's release build targets. Run it from
# a WSL terminal (WSLg shows the GUI). The console talks to the daemon over the
# local control.sock — no network, no ports exposed.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

# Locate binaries: packaged (next to this script) or repo release builds.
find_bin() {
  local name="$1"; shift
  for p in "$HERE/$name" "$@"; do [ -x "$p" ] && { echo "$p"; return; }; done
  echo ""
}
DAEMON="$(find_bin posternd "$HERE/../crates/target/release/posternd")"
APP="$(find_bin postern-console "$HERE"/*.AppImage "$HERE/../web/src-tauri/target/release/app")"
[ -n "$DAEMON" ] || { echo "posternd not found (build: cd crates && cargo build --release -p postern-daemon)"; exit 1; }
[ -n "$APP" ] || { echo "console app not found (build: cd web && pnpm tauri build)"; exit 1; }

# Runtime dir (persistent): db / vault / keyfile / control-token / sockets.
R="${POSTERN_HOME:-$HOME/.postern}"
mkdir -p "$R"
export POSTERN_DB="$R/policy.db" POSTERN_VAULT="$R/vault.postern" POSTERN_KEYFILE="$R/key" \
  POSTERN_CONTROL_TOKEN="$R/control.token" POSTERN_CONTROL_SOCK="$R/control.sock" POSTERN_DATA_SOCK="$R/data.sock"

# First run: create keyfile + empty vault + migrated db + control-token.
[ -f "$R/control.token" ] || { echo "first run — initializing $R"; "$DAEMON" init; }

# Start the daemon if not already serving.
if ! pgrep -x posternd >/dev/null; then
  rm -f "$R/control.sock" "$R/data.sock" 2>/dev/null || true
  setsid "$DAEMON" run >"$R/daemon.log" 2>&1 </dev/null &
  for _ in $(seq 1 50); do [ -S "$R/control.sock" ] && break; sleep 0.2; done
fi
[ -S "$R/control.sock" ] || { echo "daemon failed to open control.sock; see $R/daemon.log"; tail -5 "$R/daemon.log"; exit 1; }
echo "daemon up: $R/control.sock"

# Launch the desktop console. WSLg WebKitGTK needs software rendering / no DMABUF
# or the window paints black.
export WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 LIBGL_ALWAYS_SOFTWARE=1
echo "launching Postern Console ..."
exec "$APP"
