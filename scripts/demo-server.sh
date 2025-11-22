#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
state_root="$repo_root/target/demo"
state_dir="$(mktemp -d "$state_root/state-XXXXXX")"
db_path="$state_dir/pod-upgrade-trigger.db"
log_file="$repo_root/demo-http.log"
pid_file="$repo_root/demo-http.pid"

mkdir -p "$state_root"

cd "$repo_root"

export PODUP_ENV="${PODUP_ENV:-demo}"
export PATH="$repo_root/tests/mock-bin:${PATH:-}"
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

echo "[demo] state_dir=$state_dir"

echo "[demo] building backend binary"
cargo build --bin pod-upgrade-trigger

if [ ! -d "$repo_root/web/dist" ] || [ -z "$(ls -A "$repo_root/web/dist" 2>/dev/null || true)" ]; then
  echo "[demo] web/dist missing or empty, building frontend bundle"
  cd "$repo_root/web"
  if command -v bun >/dev/null 2>&1; then
    bun install --frozen-lockfile || bun install
    bun run build
  else
    npm install
    npm run build
  fi
  cd "$repo_root"
fi

echo "[demo] seeding demo data into sqlite://$db_path"
PODUP_ENV="dev" \
PODUP_STATE_DIR="$state_dir" \
PODUP_DB_URL="sqlite://$db_path" \
target/debug/pod-upgrade-trigger seed-demo

if [ -f "$pid_file" ]; then
  if ps -p "$(cat "$pid_file")" >/dev/null 2>&1; then
    echo "[demo] existing demo http-server is running with pid $(cat "$pid_file")"
    exit 0
  fi
fi

echo "[demo] starting http-server on 127.0.0.1:25311"
: >"$log_file"
PODUP_ENV="$PODUP_ENV" \
PODUP_STATE_DIR="$state_dir" \
PODUP_DB_URL="sqlite://$db_path" \
PODUP_TOKEN="${PODUP_TOKEN:-demo-token}" \
PODUP_MANUAL_TOKEN="${PODUP_MANUAL_TOKEN:-demo-token}" \
PODUP_GH_WEBHOOK_SECRET="${PODUP_GH_WEBHOOK_SECRET:-demo-secret}" \
PODUP_DEBUG_PAYLOAD_PATH="$state_dir/last_payload.bin" \
PODUP_DEV_OPEN_ADMIN="${PODUP_DEV_OPEN_ADMIN:-1}" \
PODUP_HTTP_ADDR="${PODUP_HTTP_ADDR:-127.0.0.1:25311}" \
PODUP_PUBLIC_BASE_URL="${PODUP_PUBLIC_BASE_URL:-http://127.0.0.1:25311}" \
PODUP_AUDIT_SYNC="${PODUP_AUDIT_SYNC:-1}" \
target/debug/pod-upgrade-trigger http-server >"$log_file" 2>&1 &
server_pid=$!
echo "$server_pid" >"$pid_file"

echo "[demo] http-server pid=$server_pid, log=$log_file, url=${PODUP_PUBLIC_BASE_URL:-http://127.0.0.1:25311}"
echo "[demo] seeded state_dir=$state_dir"
