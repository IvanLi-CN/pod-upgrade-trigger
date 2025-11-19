#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
log_file="$repo_root/dev-http.log"
pid_file="$repo_root/dev-http.pid"

cd "$repo_root"

export PODUP_ENV="${PODUP_ENV:-dev}"
export PATH="$repo_root/tests/mock-bin:${PATH:-}"
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

echo "[dev] building backend binary"
cargo build --bin pod-upgrade-trigger

if [ ! -d "$repo_root/web/dist" ] || [ -z "$(ls -A "$repo_root/web/dist" 2>/dev/null || true)" ]; then
  echo "[dev] web/dist missing or empty, building frontend bundle"
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

if [ -f "$pid_file" ]; then
  if ps -p "$(cat "$pid_file")" >/dev/null 2>&1; then
    echo "[dev] existing dev http-server is running with pid $(cat "$pid_file")"
    exit 0
  fi
fi

echo "[dev] starting http-server on 127.0.0.1:25111"
: >"$log_file"
PODUP_ENV="$PODUP_ENV" \
PODUP_DEV_OPEN_ADMIN="${PODUP_DEV_OPEN_ADMIN:-1}" \
PODUP_HTTP_ADDR="${PODUP_HTTP_ADDR:-127.0.0.1:25111}" \
PODUP_PUBLIC_BASE_URL="${PODUP_PUBLIC_BASE_URL:-http://127.0.0.1:25111}" \
target/debug/pod-upgrade-trigger http-server >"$log_file" 2>&1 &
server_pid=$!
echo "$server_pid" >"$pid_file"

echo "[dev] http-server pid=$server_pid, log=$log_file, url=${PODUP_PUBLIC_BASE_URL:-http://127.0.0.1:25111}"
