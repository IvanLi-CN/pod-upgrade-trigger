#!/usr/bin/env bash
set -euo pipefail

log_info() {
  echo "[INFO] $*"
}

log_error() {
  echo "[ERROR] $*" >&2
}

REPO="ivanli-cn/pod-upgrade-trigger"
TARGET_BIN="${TARGET_BIN:-"$HOME/.local/bin/pod-upgrade-trigger"}"
TARGET_DIR="$(dirname "$TARGET_BIN")"
ASSET_NAME_BIN="pod-upgrade-trigger-x86_64-unknown-linux-gnu"
ASSET_NAME_SHA="pod-upgrade-trigger-x86_64-unknown-linux-gnu.sha256"
BASE_URL="${PODUP_RELEASE_BASE_URL:-https://github.com}"

TMP_DIR=""
cleanup() {
  if [ -n "${TMP_DIR:-}" ] && [ -d "$TMP_DIR" ]; then
    rm -rf "$TMP_DIR"
  fi
}
trap cleanup EXIT

detect_current_version() {
  if [ -x "$TARGET_BIN" ]; then
    local version
    version="$("$TARGET_BIN" --version 2>/dev/null | head -n1 || true)"
    if [ -n "$version" ]; then
      echo "$version"
    else
      echo "unknown"
    fi
  else
    echo "not installed"
  fi
}

fetch_latest_tag() {
  local json
  if ! json=$(curl --fail --silent --show-error --location \
    -H "User-Agent: pod-upgrade-trigger-updater" \
    "https://api.github.com/repos/${REPO}/releases/latest"); then
    log_error "Failed to fetch latest release metadata"
    exit 1
  fi

  local tag=""
  if command -v jq >/dev/null 2>&1; then
    tag=$(echo "$json" | jq -r '.tag_name // empty')
  else
    tag=$(echo "$json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)
  fi

  if [ -z "$tag" ]; then
    log_error "Could not parse tag_name from latest release metadata"
    exit 1
  fi

  echo "$tag"
}

main() {
  local current_version release_tag
  current_version=$(detect_current_version)
  log_info "Current version: ${current_version}"

  if [ -n "${PODUP_RELEASE_TAG:-}" ]; then
    release_tag="$PODUP_RELEASE_TAG"
    log_info "Using release tag from environment: ${release_tag}"
  else
    release_tag=$(fetch_latest_tag)
    log_info "Fetched latest release tag: ${release_tag}"
  fi

  if [ ! -d "$TARGET_DIR" ]; then
    install -d "$TARGET_DIR"
  fi
  TMP_DIR=$(mktemp -d "${TARGET_DIR}/podup-update.XXXXXX")

  local binary_url sha_url
  binary_url="${BASE_URL}/${REPO}/releases/download/${release_tag}/${ASSET_NAME_BIN}"
  sha_url="${BASE_URL}/${REPO}/releases/download/${release_tag}/${ASSET_NAME_SHA}"

  log_info "Download URL (binary): ${binary_url}"
  log_info "Download URL (sha256): ${sha_url}"

  (
    cd "$TMP_DIR"
    curl --fail --silent --show-error --location -o "$ASSET_NAME_SHA" "$sha_url"
    curl --fail --silent --show-error --location -o "$ASSET_NAME_BIN" "$binary_url"
    sha256sum -c "$ASSET_NAME_SHA"
  )

  local staged_bin
  staged_bin="${TARGET_BIN}.new"

  cp "$TMP_DIR/$ASSET_NAME_BIN" "$staged_bin"
  chmod +x "$staged_bin"

  if [ -e "$TARGET_BIN" ]; then
    log_info "Backing up existing binary to ${TARGET_BIN}.old"
    if mv "$TARGET_BIN" "${TARGET_BIN}.old"; then
      log_info "Backup created at ${TARGET_BIN}.old"
    else
      log_error "Warning: failed to back up existing binary; proceeding without backup"
    fi
  fi

  mv "$staged_bin" "$TARGET_BIN"
  log_info "Replaced binary at ${TARGET_BIN}"

  if systemctl --user restart pod-upgrade-trigger-http.service; then
    log_info "Service restarted successfully"
  else
    log_error "Failed to restart pod-upgrade-trigger-http.service"
    exit 1
  fi
}

main "$@"
