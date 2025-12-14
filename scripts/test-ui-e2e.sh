#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
state_root="$repo_root/target/ui-e2e"
state_dir="$state_root/state"
auth_state_dir="$state_root/state-auth"
http_log="$repo_root/ui-e2e-http.log"
auth_http_log="$repo_root/ui-e2e-http-auth.log"
mock_log="$repo_root/tests/mock-bin/log.txt"

mkdir -p "$state_dir" "$auth_state_dir"
mkdir -p "$(dirname "$mock_log")"
: >"$mock_log"

export PATH="$repo_root/tests/mock-bin:$PATH"
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

cd "$repo_root"

echo "[ui-e2e] building backend binary"
cargo build --bin pod-upgrade-trigger

echo "[ui-e2e] installing front-end dependencies with Bun"
cd "$repo_root/web"
if command -v bun >/dev/null 2>&1; then
  export VITE_ENABLE_MOCKS="true"
  bun install --frozen-lockfile || bun install
  echo "[ui-e2e] building front-end dist with Bun"
  bun run build
else
  echo "[ui-e2e] bun is not available; please install Bun or adjust the script." >&2
  exit 1
fi

cd "$repo_root"
echo "[ui-e2e] starting http-server on 127.0.0.1:25211"
: >"$http_log"
PODUP_SKIP_PODMAN="1" \
PODUP_ENV="test" \
PODUP_STATE_DIR="$state_dir" \
PODUP_DB_URL="sqlite://$state_dir/pod-upgrade-trigger.db" \
PODUP_TOKEN="e2e-token" \
PODUP_GH_WEBHOOK_SECRET="e2e-secret" \
PODUP_MANUAL_UNITS="svc-alpha.service,svc-beta.service" \
PODUP_DEBUG_PAYLOAD_PATH="$state_dir/last_payload.bin" \
PODUP_DEV_OPEN_ADMIN="1" \
PODUP_HTTP_ADDR="127.0.0.1:25211" \
PODUP_PUBLIC_BASE_URL="http://127.0.0.1:25211" \
PODUP_AUDIT_SYNC="1" \
target/debug/pod-upgrade-trigger http-server >"$http_log" 2>&1 &
server_pid_main=$!

echo "[ui-e2e] starting auth http-server on 127.0.0.1:25212"
: >"$auth_http_log"
PODUP_SKIP_PODMAN="1" \
PODUP_ENV="test" \
PODUP_STATE_DIR="$auth_state_dir" \
PODUP_DB_URL="sqlite://$auth_state_dir/pod-upgrade-trigger.db" \
PODUP_TOKEN="e2e-token" \
PODUP_GH_WEBHOOK_SECRET="e2e-secret" \
PODUP_MANUAL_UNITS="svc-alpha.service,svc-beta.service" \
PODUP_DEBUG_PAYLOAD_PATH="$auth_state_dir/last_payload.bin" \
PODUP_DEV_OPEN_ADMIN="0" \
PODUP_FWD_AUTH_HEADER="X-Forwarded-User" \
PODUP_FWD_AUTH_ADMIN_VALUE="admin" \
PODUP_HTTP_ADDR="127.0.0.1:25212" \
PODUP_PUBLIC_BASE_URL="http://127.0.0.1:25212" \
PODUP_AUDIT_SYNC="1" \
target/debug/pod-upgrade-trigger http-server >"$auth_http_log" 2>&1 &
server_pid_auth=$!

cleanup() {
  if ps -p "${server_pid_main:-0}" >/dev/null 2>&1; then
    kill "$server_pid_main" 2>/dev/null || true
    wait "$server_pid_main" 2>/dev/null || true
  fi
  if ps -p "${server_pid_auth:-0}" >/dev/null 2>&1; then
    kill "$server_pid_auth" 2>/dev/null || true
    wait "$server_pid_auth" 2>/dev/null || true
  fi
}
trap cleanup EXIT

echo "[ui-e2e] waiting for /health"
ready_main=0
ready_auth=0

for _ in {1..60}; do
  if curl -fsS "http://127.0.0.1:25211/health" >/dev/null 2>&1; then
    ready_main=1
  fi
  if curl -fsS "http://127.0.0.1:25212/health" >/dev/null 2>&1; then
    ready_auth=1
  fi
  if [[ "$ready_main" == "1" && "$ready_auth" == "1" ]]; then
    break
  fi
  sleep 0.5
done

if [[ "$ready_main" != "1" ]]; then
  echo "[ui-e2e] http-server (main) failed to become ready; last log lines:"
  tail -n 100 "$http_log" || true
  exit 1
fi

if [[ "$ready_auth" != "1" ]]; then
  echo "[ui-e2e] http-server (auth) failed to become ready; last log lines:"
  tail -n 100 "$auth_http_log" || true
  exit 1
fi

echo "[ui-e2e] running Playwright tests"
cd "$repo_root/web"
if command -v bun >/dev/null 2>&1; then
  base_url="http://127.0.0.1:25211"
  auth_url="http://127.0.0.1:25212"
  UI_E2E_BASE_URL="$base_url" UI_E2E_AUTH_BASE_URL="$auth_url" bunx playwright test "$@"
else
  echo "[ui-e2e] bun is not available for Playwright; please install Bun." >&2
  exit 1
fi
