#!/usr/bin/env bash
set -euo pipefail

# Read the first request line from stdin (e.g., "GET /auto-update?token=XYZ HTTP/1.1")
read -r REQUEST_LINE || true

# Minimal parser for method and path
METHOD=$(awk '{print $1}' <<<"${REQUEST_LINE:-}" 2>/dev/null || echo "")
REQUEST_PATH=$(awk '{print $2}' <<<"${REQUEST_LINE:-}" 2>/dev/null || echo "")

log() { logger -t pod-upgrade-trigger -- "$*"; }

# Helper: write HTTP response
http_resp() {
  local code="$1"; shift
  local text="$1"; shift
  local body="${1:-}"
  printf "HTTP/1.1 %s %s\r\n" "$code" "$text"
  printf "Content-Type: text/plain; charset=utf-8\r\n"
  printf "Connection: close\r\n"
  printf "\r\n"
  [[ -n "$body" ]] && printf "%s\n" "$body"
}

# Rate limit guard: two windows (defaults: 2/10m and 10/5h)
rate_limit_check() {
  local now ts path db lock
  local l1_count="${PODUP_LIMIT1_COUNT:-2}"
  local l1_window="${PODUP_LIMIT1_WINDOW:-600}"
  local l2_count="${PODUP_LIMIT2_COUNT:-10}"
  local l2_window="${PODUP_LIMIT2_WINDOW:-18000}"

  # Default to generic state dir; prefer PODUP_STATE_DIR in real deployments.
  path="${PODUP_STATE_DIR:-/srv/pod-upgrade-trigger}"
  db="$path/ratelimit.db"
  lock="$path/ratelimit.lock"
  now="$(date +%s)"

  mkdir -p "$path" 2>/dev/null || true
  : > "$db" 2>/dev/null || true

  exec 200>"$lock"
  if ! flock -w 2 200; then
    log "429 rate-limit lock-timeout"
    http_resp 429 Too\ Many\ Requests "rate limited"
    exit 0
  fi

  local cutoff_l2=$((now - l2_window))
  awk -v cut="$cutoff_l2" 'NF && $1 >= cut {print $1}' "$db" >"$db.tmp" 2>/dev/null || true
  mv "$db.tmp" "$db"

  local cutoff_l1=$((now - l1_window))
  local c1 c2
  c1=$(awk -v cut="$cutoff_l1" 'NF && $1 >= cut {c++} END{print c+0}' "$db")
  c2=$(awk -v cut="$cutoff_l2" 'NF && $1 >= cut {c++} END{print c+0}' "$db")

  if [ "$c1" -ge "$l1_count" ] || [ "$c2" -ge "$l2_count" ]; then
    log "429 rate-limit c1=$c1/$l1_count c2=$c2/$l2_count"
    http_resp 429 Too\ Many\ Requests "rate limited"
    exit 0
  fi

  printf "%s\n" "$now" >>"$db"
}

# function: redact token in logs
redact_token() {
  local input="$1"
  sed -E 's/(token=)[^& ]+/\1***REDACTED***/g' <<<"$input"
}

# Health endpoint
if [[ "$METHOD" == "GET" && "$REQUEST_PATH" == "/health" ]]; then
  log "health ok"
  http_resp 200 OK "ok"
  exit 0
fi

# Only accept GET/POST on /auto-update with a token
if [[ "$REQUEST_PATH" != *"/auto-update"* ]]; then
  log "404 $(redact_token "$REQUEST_LINE")"
  http_resp 404 NotFound "not found"
  exit 0
fi

# Extract token from query string
TOKEN_QUERY=$(sed -n 's/.*token=\([^& ]*\).*/\1/p' <<<"$REQUEST_PATH" || true)
REQ_TOKEN="${TOKEN_QUERY:-}"

if [[ -z "${PODUP_TOKEN:-}" || -z "$REQ_TOKEN" || "$REQ_TOKEN" != "$PODUP_TOKEN" ]]; then
  log "401 $(redact_token "$REQUEST_LINE")"
  http_resp 401 Unauthorized "unauthorized"
  exit 0
fi

rate_limit_check

# Fire and forget the auto-update in the user systemd session
if systemctl --user start podman-auto-update.service >/dev/null 2>&1; then
  log "202 triggered $(redact_token "$REQUEST_LINE")"
  http_resp 202 Accepted "auto-update triggered"
else
  log "500 failed $(redact_token "$REQUEST_LINE")"
  http_resp 500 InternalServerError "failed to trigger"
fi
