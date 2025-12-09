#!/usr/bin/env bash
set -euo pipefail

# Default to a generic state dir; override via PODUP_STATE_DIR in real deployments.
DEFAULT_STATE_DIR="/srv/pod-upgrade-trigger"
REPORT_SUBDIR="self-update-reports"
DRY_RUN="false"

parse_bool() {
  local value="${1:-}"
  value="${value,,}"
  case "$value" in
  1 | true | yes | on)
    echo "true"
    ;;
  0 | false | no | off | "")
    echo "false"
    ;;
  *)
    return 1
    ;;
  esac
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
update_script="${script_dir}/update-pod-upgrade-trigger-from-release.sh"

state_dir="${PODUP_STATE_DIR:-$DEFAULT_STATE_DIR}"
report_dir="${PODUP_SELF_UPDATE_REPORT_DIR:-}"
if [ -z "$report_dir" ]; then
  report_dir="${state_dir}/${REPORT_SUBDIR}"
fi

if [ -z "${report_dir// }" ]; then
  echo "[ERROR] report directory is empty; set PODUP_STATE_DIR or PODUP_SELF_UPDATE_REPORT_DIR" >&2
  exit 1
fi

mkdir -p "$report_dir"

if [ -n "${PODUP_SELF_UPDATE_DRY_RUN:-}" ]; then
  DRY_RUN=$(parse_bool "$PODUP_SELF_UPDATE_DRY_RUN" || echo "false")
fi

while [ "$#" -gt 0 ]; do
  case "$1" in
  --dry-run)
    DRY_RUN="true"
    ;;
  -h | --help)
    echo "Usage: self-update-runner.sh [--dry-run]" >&2
    exit 0
    ;;
  *)
    echo "[ERROR] Unknown argument: $1" >&2
    exit 2
    ;;
  esac
  shift
done

started_at="$(date +%s)"
stderr_tail=""
exit_code=0
status="succeeded"

if [ ! -x "$update_script" ]; then
  stderr_tail="update script not found or not executable: ${update_script}"
  echo "[ERROR] $stderr_tail" >&2
  exit_code=127
  status="failed"
else
  stderr_file="$(mktemp "${report_dir}/self-update-${started_at}-$$.stderr.XXXXXX")"

  set +e
  if [ "$DRY_RUN" = "true" ]; then
    PODUP_SELF_UPDATE_DRY_RUN=1 "$update_script" --dry-run 2>"$stderr_file"
  else
    "$update_script" 2>"$stderr_file"
  fi
  exit_code=$?
  set -e

  if [ "$exit_code" -ne 0 ]; then
    status="failed"
  fi

  if [ -s "$stderr_file" ]; then
    stderr_tail="$(tail -n 40 "$stderr_file")"
  fi
  rm -f "$stderr_file"
fi

finished_at="$(date +%s)"

binary_path="${TARGET_BIN:-"$HOME/.local/bin/pod-upgrade-trigger"}"
release_tag="${PODUP_RELEASE_TAG:-}"
runner_host="$(hostname 2>/dev/null || echo "unknown")"

export PODUP_STARTED_AT="$started_at"
export PODUP_FINISHED_AT="$finished_at"
export PODUP_STATUS="$status"
export PODUP_EXIT_CODE="$exit_code"
export PODUP_BINARY_PATH="$binary_path"
export PODUP_RELEASE_TAG="$release_tag"
export PODUP_STDERR_TAIL="$stderr_tail"
export PODUP_RUNNER_HOST="$runner_host"
export PODUP_RUNNER_PID="$$"
export PODUP_DRY_RUN="$DRY_RUN"

report_json="$(python3 - <<'PY'
import json
import os

def optional(key):
    value = os.environ.get(key, "")
    return value if value else None

dry_run_env = os.environ.get("PODUP_DRY_RUN", "false").lower()
dry_run = dry_run_env in ("1", "true", "yes", "on")

report = {
    "type": "self-update-run",
    "dry_run": dry_run,
    "started_at": int(os.environ["PODUP_STARTED_AT"]),
    "finished_at": int(os.environ["PODUP_FINISHED_AT"]),
    "status": os.environ["PODUP_STATUS"],
    "exit_code": int(os.environ["PODUP_EXIT_CODE"]),
    "binary_path": optional("PODUP_BINARY_PATH"),
    "release_tag": optional("PODUP_RELEASE_TAG"),
    "stderr_tail": optional("PODUP_STDERR_TAIL"),
    "runner_host": optional("PODUP_RUNNER_HOST"),
    "runner_pid": int(os.environ["PODUP_RUNNER_PID"]),
}

print(json.dumps(report))
PY
)"

base_name="self-update-${started_at}-$$"
tmp_path="${report_dir}/${base_name}.json.tmp"
final_path="${report_dir}/${base_name}.json"

printf '%s\n' "$report_json" >"$tmp_path"
mv "$tmp_path" "$final_path"

exit "$exit_code"
