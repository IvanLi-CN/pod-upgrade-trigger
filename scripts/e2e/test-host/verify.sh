#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  scripts/e2e/test-host/verify.sh root@<host> [--user ivan]

Checks acceptance criteria for the test host baseline:
  - docker active + docker version
  - user exists + key-based SSH login works
  - systemd user units accessible in non-interactive SSH
  - rootless podman works
  - quadlet dir exists and owned by user
  - auto-update log dir NOT pre-created
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || "${1:-}" == "" ]]; then
  usage
  exit 1
fi

root_target="$1"
shift

test_user="ivan"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --user)
      test_user="$2"
      shift 2
      ;;
    *)
      echo "[verify] unknown arg: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

ssh_opts=(
  -o BatchMode=yes
  -o StrictHostKeyChecking=accept-new
)

ssh_root() {
  local cmd="$1"
  printf '%s\n' "set -euo pipefail" "$cmd" | ssh "${ssh_opts[@]}" "$root_target" -- bash -s
}

ssh_user() {
  local cmd="$1"
  printf '%s\n' "set -euo pipefail" "$cmd" | ssh "${ssh_opts[@]}" "${test_user}@${root_target#*@}" -- bash -s
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
    fail=$((fail + 1))
  fi
}

echo "[verify] target=$root_target user=$test_user"

check "docker version returns 0" "ssh_root 'docker version'"
check "docker service active" "ssh_root 'systemctl is-active --quiet docker'"
check "user can SSH login (id)" "ssh_user 'id'"
check "systemctl --user works (no bus error)" "ssh_user 'systemctl --user list-units --no-pager >/dev/null'"
check "podman --version returns 0" "ssh_user 'podman --version'"
check "quadlet dir exists" "ssh_root 'test -d /home/$test_user/.config/containers/systemd'"
check "quadlet dir owned by user" "ssh_root 'stat -c %U /home/$test_user/.config/containers/systemd | grep -qx \"$test_user\"'"
check "auto-update log dir NOT pre-created" "ssh_root 'test ! -e /home/$test_user/.local/share/podman-auto-update/logs'"

port_2222_status="$(ssh_root 'if command -v ss >/dev/null 2>&1; then if ss -ltnp 2>/dev/null | grep -q ":2222"; then echo "2222 in use"; else echo "2222 free"; fi; else echo "ss not found"; fi')"
echo "[verify] host port check: $port_2222_status"

echo "[verify] pass=$pass fail=$fail"
if [[ "$fail" -ne 0 ]]; then
  exit 1
fi
