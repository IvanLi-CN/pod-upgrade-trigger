#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

staged_files="$(git diff --cached --name-only --diff-filter=ACM || true)"
if [ -z "${staged_files}" ]; then
  exit 0
fi

# Rust: format only staged *.rs files (including build.rs) and re-stage via lefthook `stage_fixed`.
rust_files="$(printf '%s\n' "$staged_files" | grep -E '\\.rs$' || true)"
if [ -n "${rust_files}" ]; then
  if ! command -v rustfmt >/dev/null 2>&1; then
    echo "[fmt] rustfmt not found; install rustfmt via rustup" >&2
    exit 1
  fi

  echo "$rust_files" | xargs -I{} rustfmt --edition 2024 {}
fi

# Web: format staged files under web/ with Biome and re-stage via lefthook `stage_fixed`.
web_files="$(printf '%s\n' "$staged_files" | grep -E '^web/.*\\.(ts|tsx|js|jsx|json|html)$' || true)"
if [ -n "${web_files}" ]; then
  if [ ! -x "web/node_modules/.bin/biome" ]; then
    echo "[fmt] biome not found at web/node_modules/.bin/biome" >&2
    echo "[fmt] run: (cd web && bun install || npm install)" >&2
    exit 1
  fi

  web_files_rel="$(printf '%s\n' "$web_files" | sed 's|^web/||')"
  (
    cd web
    echo "$web_files_rel" | xargs ./node_modules/.bin/biome format --write
  )
fi
