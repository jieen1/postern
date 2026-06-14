#!/usr/bin/env bash
# Automated per-page verification of the web frontend against the REAL backend,
# headless (no GUI, no human). Path: chromium -> vite(web) -> /v1 proxy ->
# control-bridge -> control.sock -> posternd. Asserts every nav page renders
# without crashing (error boundary / uncaught exception).
#
# Prereq: posternd running, with POSTERN_CONTROL_SOCK + POSTERN_CONTROL_TOKEN
# exported (e.g. the ~/.postern-live runtime dir). Usage: bash tools/run-verify.sh
set -euo pipefail
cd "$(dirname "$0")/.."

: "${POSTERN_CONTROL_SOCK:?export POSTERN_CONTROL_SOCK (daemon control socket)}"
: "${POSTERN_CONTROL_TOKEN:?export POSTERN_CONTROL_TOKEN (control-token file)}"
BRIDGE_PORT="${BRIDGE_PORT:-8787}"
PORT="${VERIFY_PORT:-5174}"

BRIDGE_PORT="$BRIDGE_PORT" node tools/control-bridge.mjs &
BPID=$!
VITE_TARGET=web VITE_API_PROXY="http://127.0.0.1:${BRIDGE_PORT}" \
  pnpm exec vite --port "$PORT" --strictPort &
VPID=$!
trap 'kill "$BPID" "$VPID" 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
  curl -s -o /dev/null "http://localhost:${PORT}" && break
  sleep 1
done

VERIFY_BASE="http://localhost:${PORT}" node tools/verify-pages.mjs
