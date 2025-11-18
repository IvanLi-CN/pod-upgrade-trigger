#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
state_root="$repo_root/target/ui-e2e"
state_dir="$state_root/state"
http_log="$repo_root/ui-e2e-http.log"
mock_log="$repo_root/tests/mock-bin/log.txt"

mkdir -p "$state_dir"
mkdir -p "$(dirname "$mock_log")"
: >"$mock_log"

export PATH="$repo_root/tests/mock-bin:$PATH"
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

export WEBHOOK_STATE_DIR="$state_dir"
export WEBHOOK_DB_URL="sqlite://$state_dir/pod-upgrade-trigger.db"
export WEBHOOK_WEB_DIST="$repo_root/web/dist"
export WEBHOOK_TOKEN="e2e-token"
export WEBHOOK_MANUAL_TOKEN="e2e-token"
export GITHUB_WEBHOOK_SECRET="e2e-secret"
export WEBHOOK_MANUAL_UNITS="svc-alpha.service,svc-beta.service"
export WEBHOOK_DEBUG_PAYLOAD_PATH="$state_dir/last_payload.bin"
export DEV_OPEN_ADMIN="1"
export WEBHOOK_HTTP_ADDR="${WEBHOOK_HTTP_ADDR:-127.0.0.1:25211}"
export WEBHOOK_PUBLIC_BASE_URL="http://127.0.0.1:25211"
export WEBHOOK_AUDIT_SYNC="1"

cd "$repo_root"

echo "[ui-e2e] building backend binary"
cargo build --bin webhook-auto-update

echo "[ui-e2e] installing front-end dependencies with Bun"
cd "$repo_root/web"
if command -v bun >/dev/null 2>&1; then
  bun install --frozen-lockfile || bun install
  echo "[ui-e2e] building front-end dist with Bun"
  bun run build
else
  echo "[ui-e2e] bun is not available; please install Bun or adjust the script." >&2
  exit 1
fi

cd "$repo_root"
echo "[ui-e2e] starting http-server on $WEBHOOK_HTTP_ADDR"
: >"$http_log"
target/debug/webhook-auto-update http-server >"$http_log" 2>&1 &
server_pid=$!

cleanup() {
  if ps -p "$server_pid" >/dev/null 2>&1; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

echo "[ui-e2e] waiting for /health"
for _ in {1..60}; do
  if curl -fsS "http://127.0.0.1:25211/health" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.5
done

if [[ "${ready:-0}" != "1" ]]; then
  echo "[ui-e2e] http-server failed to become ready; last log lines:"
  tail -n 100 "$http_log" || true
  exit 1
fi

echo "[ui-e2e] running Playwright tests"
cd "$repo_root/web"
if command -v bun >/dev/null 2>&1; then
  UI_E2E_BASE_URL="http://127.0.0.1:25211" bunx playwright test "$@"
else
  echo "[ui-e2e] bun is not available for Playwright; please install Bun." >&2
  exit 1
fi
