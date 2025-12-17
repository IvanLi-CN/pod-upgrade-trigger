#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

usage() {
  cat <<'USAGE'
Usage:
  scripts/test-e2e-ssh.sh root@<host>

Runs backend E2E against a real SSH target running on the remote test host.

This script:
  - verifies the test host baseline
  - (re)deploys the SSH target container (podup-test, 2222:22)
  - runs cargo tests in tests/e2e_ssh.rs (no tests/mock-bin PATH injection)

Requirements:
  - SSH key access to root@<host> and ivan@<host>
  - OpenSSH client on the dev machine
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || "${1:-}" == "" ]]; then
  usage
  exit 1
fi

root_target="$1"
host="${root_target#*@}"
shift

lock_dir="${TMPDIR:-/tmp}/podup-e2e-ssh-${host}.lock"
if ! mkdir "$lock_dir" 2>/dev/null; then
  echo "[e2e-ssh] ERROR: lock exists ($lock_dir); another SSH E2E run may be in progress" >&2
  exit 1
fi
trap 'rmdir "$lock_dir" 2>/dev/null || true' EXIT

export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

echo "[e2e-ssh] verifying test host baseline: $root_target"
./scripts/e2e/test-host/verify.sh "$root_target"

echo "[e2e-ssh] deploying SSH target container (podup-test, 2222:22)"
./scripts/e2e/ssh-target/deploy.sh "$root_target"
./scripts/e2e/ssh-target/verify.sh "$root_target"

export PODUP_E2E_SSH=1
export PODUP_SSH_TARGET="ssh://ivan@${host}:2222"
export PODUP_CONTAINER_DIR="/home/ivan/.config/containers/systemd"
export PODUP_AUTO_UPDATE_LOG_DIR="/home/ivan/.local/share/podup-e2e/logs-missing"

echo "[e2e-ssh] running: cargo test --locked --test e2e_ssh -- --nocapture"
echo "[e2e-ssh] PODUP_SSH_TARGET=$PODUP_SSH_TARGET"
echo "[e2e-ssh] PODUP_CONTAINER_DIR=$PODUP_CONTAINER_DIR"
echo "[e2e-ssh] PODUP_AUTO_UPDATE_LOG_DIR=$PODUP_AUTO_UPDATE_LOG_DIR"

cargo test --locked --test e2e_ssh -- --nocapture "$@"
