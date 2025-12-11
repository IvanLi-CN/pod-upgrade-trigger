#!/usr/bin/env bash
set -euo pipefail

# Compute effective semver from Cargo.toml, auto-increment patch if tag already exists.
root_dir=$(git rev-parse --show-toplevel)

# Ensure tags are available even if checkout depth is shallow.
git fetch --tags --force >/dev/null 2>&1 || true

current_commit=$(git rev-parse HEAD)

cargo_ver=$(grep -m1 '^version\s*=\s*"' "$root_dir/Cargo.toml" | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/')
if [[ -z "$cargo_ver" ]]; then
  echo "Failed to detect version from Cargo.toml" >&2
  exit 1
fi

existing_tag=$(git tag --points-at "$current_commit" | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -n1 || true)
if [[ -n "$existing_tag" ]]; then
  effective="${existing_tag#v}"
  echo "APP_EFFECTIVE_VERSION=${effective}" >> "$GITHUB_ENV"
  echo "Computed APP_EFFECTIVE_VERSION=${effective} from existing tag ${existing_tag}"
  exit 0
fi

base_major=${cargo_ver%%.*}
rest=${cargo_ver#*.}
base_minor=${rest%%.*}
base_patch=${cargo_ver##*.}

candidate="$base_patch"
while git rev-parse -q --verify "refs/tags/v${base_major}.${base_minor}.${candidate}" >/dev/null; do
  candidate=$((candidate + 1))
done

effective="${base_major}.${base_minor}.${candidate}"
echo "APP_EFFECTIVE_VERSION=${effective}" >> "$GITHUB_ENV"
echo "Computed APP_EFFECTIVE_VERSION=${effective} (base ${cargo_ver})"
