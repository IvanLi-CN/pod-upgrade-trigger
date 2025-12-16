#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  scripts/e2e/ssh-target/verify.sh root@<host>

Verifies the Docker SSH target container (podup-test) is usable as an SSH host
backend:
  - SSH works (key-only) on host port 2222
  - systemctl --user works in non-interactive SSH
  - podman present
  - podup-e2e-noop.service can be controlled
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || "${1:-}" == "" ]]; then
  usage
  exit 1
fi

root_target="$1"
host="${root_target#*@}"

ops_user="ivan"
container_name="podup-test"
host_port="2222"

ssh_opts=(
  -o BatchMode=yes
  -o StrictHostKeyChecking=accept-new
  -o ConnectTimeout=5
  -o ConnectionAttempts=1
)

ssh_root() {
  local cmd="$1"
  printf '%s\n' "set -euo pipefail" "$cmd" | ssh "${ssh_opts[@]}" "$root_target" -- bash -s
}

ssh_target() {
  local cmd="$1"
  printf '%s\n' "set -euo pipefail" "$cmd" | ssh "${ssh_opts[@]}" -p "$host_port" "${ops_user}@${host}" -- bash -s
}

pass=0
fail=0

check() {
  local name="$1"
  local cmd="$2"
  if eval "$cmd" >/dev/null 2>&1; then
    echo "[verify] PASS: $name"
    pass=$((pass + 1))
  else
    echo "[verify] FAIL: $name" >&2
    echo "[verify] ---- command ----" >&2
    echo "$cmd" >&2
    echo "[verify] ---- output ----" >&2
    # Re-run without redirect so the user sees why it failed.
    eval "$cmd" >&2 || true
    fail=$((fail + 1))
  fi
}

echo "[verify] root_target=$root_target host=$host port=${host_port} user=$ops_user container_name=$container_name"

check "container running (docker/podman inspect)" "ssh_root 'docker inspect -f \"{{.State.Running}}\" \"$container_name\" 2>/dev/null | grep -qx true || podman inspect -f \"{{.State.Running}}\" \"$container_name\" 2>/dev/null | grep -qx true'"
check "ssh login works (id)" "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=5 -o ConnectionAttempts=1 -p \"$host_port\" \"${ops_user}@${host}\" -- \"id\""

check "systemctl --user works (no bus error)" "ssh_target 'systemctl --user list-units --no-pager >/dev/null'"
check "podman --version returns 0" "ssh_target 'podman --version'"

check "daemon-reload ok" "ssh_target 'systemctl --user daemon-reload'"
check "enable --now podup-e2e-noop.service" "ssh_target 'systemctl --user enable --now --no-block podup-e2e-noop.service; for i in {1..30}; do systemctl --user is-active --quiet podup-e2e-noop.service && exit 0; sleep 1; done; systemctl --user status podup-e2e-noop.service --no-pager || true; exit 1'"
check "restart podup-e2e-noop.service" "ssh_target 'systemctl --user restart --no-block podup-e2e-noop.service; for i in {1..30}; do systemctl --user is-active --quiet podup-e2e-noop.service && exit 0; sleep 1; done; systemctl --user status podup-e2e-noop.service --no-pager || true; exit 1'"
check "stop podup-e2e-noop.service" "ssh_target 'systemctl --user stop --no-block podup-e2e-noop.service; for i in {1..30}; do systemctl --user is-active --quiet podup-e2e-noop.service || exit 0; sleep 1; done; systemctl --user status podup-e2e-noop.service --no-pager || true; exit 1'"

echo "[verify] pass=$pass fail=$fail"
if [[ "$fail" -ne 0 ]]; then
  exit 1
fi

echo "[verify] PASS"
