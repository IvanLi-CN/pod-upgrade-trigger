#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
log_file="$repo_root/ui-e2e-mock.log"

cd "$repo_root/web"

echo "[ui-e2e-mock] installing front-end dependencies with Bun"
if command -v bun >/dev/null 2>&1; then
  export VITE_ENABLE_MOCKS="true"
  bun install --frozen-lockfile || bun install
  echo "[ui-e2e-mock] building front-end dist with Bun"
  bun run build
else
  echo "[ui-e2e-mock] bun is not available; please install Bun or adjust the script." >&2
  exit 1
fi

cd "$repo_root/web"
: >"$log_file"

echo "[ui-e2e-mock] starting Vite preview with mocks on 127.0.0.1:25211"
VITE_ENABLE_MOCKS=true bunx vite preview --host 127.0.0.1 --port 25211 --strictPort >"$log_file" 2>&1 &
server_pid=$!

cleanup() {
  if ps -p "${server_pid:-0}" >/dev/null 2>&1; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

# wait for preview server
for _ in {1..30}; do
  if curl -fsS "http://127.0.0.1:25211/" >/dev/null 2>&1; then
    break
  fi
  sleep 0.5
done

if ! curl -fsS "http://127.0.0.1:25211/" >/dev/null 2>&1; then
  echo "[ui-e2e-mock] preview failed to become ready; last log lines:" >&2
  tail -n 80 "$log_file" || true
  exit 1
fi

echo "[ui-e2e-mock] running Playwright tests in mock mode"
UI_E2E_BASE_URL="http://127.0.0.1:25211" UI_E2E_AUTH_BASE_URL="http://127.0.0.1:25211" bunx playwright test "$@"
