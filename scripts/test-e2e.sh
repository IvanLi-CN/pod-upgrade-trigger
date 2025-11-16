#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
log_file="$repo_root/tests/mock-bin/log.txt"
mkdir -p "$(dirname "$log_file")"
: >"$log_file"
export PATH="$repo_root/tests/mock-bin:$PATH"
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
cd "$repo_root"
echo "[e2e] running with PATH=$PATH"
cargo test --locked --test e2e -- --nocapture "$@"
