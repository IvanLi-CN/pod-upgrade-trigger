#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
log_file="$repo_root/test-http.log"
pid_file="$repo_root/test-http.pid"

cd "$repo_root"

export PODUP_ENV="test"
export PATH="$repo_root/tests/mock-bin:${PATH:-}"
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

echo "[test] building backend binary"
cargo build --bin pod-upgrade-trigger

if [ -f "$pid_file" ]; then
  if ps -p "$(cat "$pid_file")" >/dev/null 2>&1; then
    echo "[test] existing test http-server is running with pid $(cat "$pid_file")"
    exit 0
  fi
fi

echo "[test] starting http-server on 127.0.0.1:25211 using in-memory DB"
: >"$log_file"
PODUP_ENV="$PODUP_ENV" \
PODUP_DEV_OPEN_ADMIN="${PODUP_DEV_OPEN_ADMIN:-1}" \
PODUP_HTTP_ADDR="${PODUP_HTTP_ADDR:-127.0.0.1:25211}" \
PODUP_PUBLIC_BASE_URL="${PODUP_PUBLIC_BASE_URL:-http://127.0.0.1:25211}" \
target/debug/pod-upgrade-trigger http-server >"$log_file" 2>&1 &
server_pid=$!
echo "$server_pid" >"$pid_file"

echo "[test] http-server pid=$server_pid, log=$log_file, url=${PODUP_PUBLIC_BASE_URL:-http://127.0.0.1:25211}"
