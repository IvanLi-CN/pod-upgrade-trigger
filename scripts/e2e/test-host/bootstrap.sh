#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/e2e/test-host/bootstrap.sh root@<host> [--user ivan] [--ssh-public-key ~/.ssh/id_ed25519.pub]

What this does (idempotent-ish):
  - Detects OS/package manager (no guessing).
  - Installs Docker Engine (distro package manager where available; otherwise get.docker.com convenience script).
  - Enables and starts docker systemd service.
  - Creates a normal user (default: ivan), adds to docker group.
  - Installs rootless Podman prerequisites and enables systemd user (linger).
  - Ensures /etc/subuid and /etc/subgid contain an entry for the user.
  - Creates Quadlet entry directory: /home/<user>/.config/containers/systemd
  - Checks whether host port 2222 is free and prints the chosen default port for later SSH target container mapping.

Notes:
  - Requires passwordless root SSH access (BatchMode=yes).
  - Does NOT create PODUP_AUTO_UPDATE_LOG_DIR and does NOT run podman auto-update.
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || "${1:-}" == "" ]]; then
  usage
  exit 1
fi

root_target="$1"
shift

test_user="ivan"
ssh_public_key_path="${HOME}/.ssh/id_ed25519.pub"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --user)
      test_user="$2"
      shift 2
      ;;
    --ssh-public-key)
      ssh_public_key_path="$2"
      shift 2
      ;;
    *)
      echo "[bootstrap] unknown arg: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ ! -f "$ssh_public_key_path" ]]; then
  echo "[bootstrap] missing public key: $ssh_public_key_path" >&2
  echo "[bootstrap] expected something like: ~/.ssh/id_ed25519.pub" >&2
  exit 3
fi

ssh_opts=(
  -o BatchMode=yes
  -o StrictHostKeyChecking=accept-new
)

ssh_root() {
  local cmd="$1"
  # NOTE: OpenSSH client concatenates argv into a single remote command string, so
  # we avoid `bash -lc "$cmd"` because quoting would be lost on the remote side.
  printf '%s\n' "set -euo pipefail" "$cmd" | ssh "${ssh_opts[@]}" "$root_target" -- bash -s
}

ssh_user() {
  local cmd="$1"
  printf '%s\n' "set -euo pipefail" "$cmd" | ssh "${ssh_opts[@]}" "${test_user}@${root_target#*@}" -- bash -s
}

pubkey="$(cat "$ssh_public_key_path")"
if [[ -z "$pubkey" ]]; then
  echo "[bootstrap] public key file is empty: $ssh_public_key_path" >&2
  exit 4
fi
pubkey_quoted="$(printf '%q' "$pubkey")"

echo "[bootstrap] repo_root=$repo_root"
echo "[bootstrap] target=$root_target user=$test_user pubkey=$ssh_public_key_path"

echo "[bootstrap] A) OS identification"
ssh "${ssh_opts[@]}" "$root_target" -- "cat /etc/os-release && uname -a"

echo "[bootstrap] detecting package manager"
pkg_mgr="$(
  ssh_root '
if command -v apt-get >/dev/null 2>&1; then echo apt; exit 0; fi
if command -v dnf >/dev/null 2>&1; then echo dnf; exit 0; fi
if command -v yum >/dev/null 2>&1; then echo yum; exit 0; fi
if command -v zypper >/dev/null 2>&1; then echo zypper; exit 0; fi
if command -v pacman >/dev/null 2>&1; then echo pacman; exit 0; fi
echo unknown
'
)"
echo "[bootstrap] pkg_mgr=$pkg_mgr"
if [[ "$pkg_mgr" == "unknown" ]]; then
  echo "[bootstrap] unsupported OS (no apt/dnf/yum/zypper/pacman found)" >&2
  exit 10
fi

if [[ "$pkg_mgr" == "pacman" ]]; then
  echo "[bootstrap] clearing stale pacman db lock (if any)"
  ssh_root '
if [ -e /var/lib/pacman/db.lck ] && ! pgrep -x pacman >/dev/null 2>&1; then
  rm -f /var/lib/pacman/db.lck
fi
'
  echo "[bootstrap] ensuring pacman keyring is initialized"
  ssh_root '
if [ ! -d /etc/pacman.d/gnupg ] || [ ! -f /etc/pacman.d/gnupg/pubring.gpg ]; then
  pacman-key --init
  pacman-key --populate archlinux
fi
'
  echo "[bootstrap] pacman full upgrade (Arch requires no partial upgrades; skipped if not needed)"
  ssh_root '
if ! command -v docker >/dev/null 2>&1 || ! command -v podman >/dev/null 2>&1; then
  pacman -Syu --noconfirm --disable-sandbox || true
fi
'
fi

echo "[bootstrap] ensuring curl is present (required for Docker install)"
case "$pkg_mgr" in
  apt)
    ssh_root 'command -v curl >/dev/null 2>&1 || (apt-get update && apt-get install -y curl ca-certificates)'
    ;;
  dnf)
    ssh_root 'command -v curl >/dev/null 2>&1 || dnf -y install curl ca-certificates'
    ;;
  yum)
    ssh_root 'command -v curl >/dev/null 2>&1 || yum -y install curl ca-certificates'
    ;;
  zypper)
    ssh_root 'command -v curl >/dev/null 2>&1 || (zypper --non-interactive refresh && zypper --non-interactive install -y curl ca-certificates)'
    ;;
  pacman)
    ssh_root 'command -v curl >/dev/null 2>&1 || pacman -Sy --noconfirm --disable-sandbox --needed curl ca-certificates'
    ;;
esac

echo "[bootstrap] B) Install and enable Docker"
case "$pkg_mgr" in
  pacman)
    ssh_root '
if ! command -v docker >/dev/null 2>&1; then
  echo "[remote] docker not found, installing via pacman (Arch official repos)"
  pacman -Sy --noconfirm --disable-sandbox --needed docker
else
  echo "[remote] docker already present: $(docker --version || true)"
fi
systemctl enable --now docker
systemctl is-active docker
'
    ;;
  *)
    ssh_root '
if ! command -v docker >/dev/null 2>&1; then
  echo "[remote] docker not found, installing via get.docker.com (official Docker convenience script)"
  curl -fsSL https://get.docker.com -o /tmp/get-docker.sh
  sh /tmp/get-docker.sh
else
  echo "[remote] docker already present: $(docker --version || true)"
fi
systemctl enable --now docker
systemctl is-active docker
'
    ;;
esac

echo "[bootstrap] C) Create user and configure SSH authorized_keys"
ssh_root "
if ! id -u '$test_user' >/dev/null 2>&1; then
  useradd -m -s /bin/bash '$test_user'
else
  echo \"[remote] user exists: $test_user\"
fi

if ! getent group docker >/dev/null 2>&1; then
  groupadd docker
fi
usermod -aG docker '$test_user'

home_dir=\"/home/$test_user\"
install -d -o '$test_user' -g '$test_user' -m 700 \"\$home_dir/.ssh\"
touch \"\$home_dir/.ssh/authorized_keys\"
chown '$test_user:$test_user' \"\$home_dir/.ssh/authorized_keys\"
chmod 600 \"\$home_dir/.ssh/authorized_keys\"
"

echo "[bootstrap] installing authorized_keys entry (stdin, idempotent)"
ssh_root "
home_dir=\"/home/$test_user\"
key=$pubkey_quoted
if ! grep -qxF \"\$key\" \"\$home_dir/.ssh/authorized_keys\"; then
  echo \"\$key\" >>\"\$home_dir/.ssh/authorized_keys\"
fi
chown '$test_user:$test_user' \"\$home_dir/.ssh/authorized_keys\"
chmod 600 \"\$home_dir/.ssh/authorized_keys\"
"

echo "[bootstrap] verifying SSH login for user ($test_user)"
ssh "${ssh_opts[@]}" "${test_user}@${root_target#*@}" -- "id && whoami"

echo "[bootstrap] D) Install and verify rootless Podman + systemd user"
case "$pkg_mgr" in
  apt)
    ssh_root '
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y podman uidmap slirp4netns fuse-overlayfs crun dbus-user-session
'
    ;;
  dnf)
    ssh_root '
set -euo pipefail
dnf -y install podman uidmap slirp4netns fuse-overlayfs crun || dnf -y install podman uidmap slirp4netns fuse-overlayfs
'
    ;;
  yum)
    ssh_root '
set -euo pipefail
yum -y install podman uidmap slirp4netns fuse-overlayfs crun || yum -y install podman uidmap slirp4netns fuse-overlayfs
'
    ;;
  zypper)
    ssh_root '
set -euo pipefail
zypper --non-interactive refresh
zypper --non-interactive install -y podman uidmap slirp4netns fuse-overlayfs crun
'
    ;;
  pacman)
    ssh_root '
# Arch: newuidmap/newgidmap are provided by the `shadow` package (no separate `uidmap` package).
if command -v podman >/dev/null 2>&1 \
  && command -v slirp4netns >/dev/null 2>&1 \
  && command -v fuse-overlayfs >/dev/null 2>&1 \
  && command -v crun >/dev/null 2>&1 \
  && command -v newuidmap >/dev/null 2>&1 \
  && command -v newgidmap >/dev/null 2>&1; then
  echo "[remote] podman/rootless deps already present"
else
  pacman -Sy --noconfirm --disable-sandbox --needed podman shadow slirp4netns fuse-overlayfs crun
fi
'
    ;;
esac

ssh_root "
ensure_subid() {
  local file=\"\$1\"
  local user=\"\$2\"
  local count=65536
  if grep -q \"^\\\$user:\" \"\\\$file\" 2>/dev/null; then
    return 0
  fi

  local start=100000
  if [ -s \"\\\$file\" ]; then
    local max_end
    max_end=\$(awk -F: 'NF>=3 {e=\$2+\$3; if (e>m) m=e} END {print m+0}' \"\\\$file\")
    if [ \"\\\$max_end\" -gt \"\\\$start\" ]; then
      start=\"\\\$max_end\"
    fi
  fi
  echo \"\\\$user:\\\$start:\\\$count\" >>\"\\\$file\"
}

ensure_subid /etc/subuid '$test_user'
ensure_subid /etc/subgid '$test_user'

loginctl enable-linger '$test_user'
"

echo "[bootstrap] non-interactive systemd user checks"
if ! ssh_user 'systemctl --user list-units --no-pager >/dev/null'; then
  echo "[bootstrap] systemctl --user failed; attempting to ensure sshd uses PAM + pam_systemd (required for XDG_RUNTIME_DIR/user bus)"
  ssh_root '
set -euo pipefail
sshd_cfg=/etc/ssh/sshd_config
if [ -f "$sshd_cfg" ]; then
  if grep -Eq "^[[:space:]]*UsePAM[[:space:]]+no" "$sshd_cfg"; then
    sed -i "s/^[[:space:]]*UsePAM[[:space:]]\\+no/UsePAM yes/" "$sshd_cfg"
  elif ! grep -Eq "^[[:space:]]*UsePAM[[:space:]]+yes" "$sshd_cfg"; then
    printf "\nUsePAM yes\n" >>"$sshd_cfg"
  fi
fi

pam_sshd=/etc/pam.d/sshd
if [ -f "$pam_sshd" ]; then
  if ! grep -q "pam_systemd\\.so" "$pam_sshd"; then
    printf "\n# Added by pod-upgrade-trigger E2E bootstrap (required for systemd --user over SSH)\nsession    required     pam_systemd.so\n" >>"$pam_sshd"
  fi
fi

systemctl reload-or-restart sshd || systemctl reload-or-restart ssh || true
'
  ssh_user 'systemctl --user list-units --no-pager >/dev/null'
fi
ssh_user 'set +o pipefail; systemctl --user list-units --no-pager | head -n 5'
ssh_user 'journalctl --user -n 1 --no-pager || true'

echo "[bootstrap] ensuring newuidmap/newgidmap are usable for rootless podman"
ssh_root '
set -euo pipefail
for f in /usr/bin/newuidmap /usr/bin/newgidmap; do
  if [ -x "$f" ]; then
    # Rootless user namespaces need setuid (or suitable filecaps); enforce setuid for test host baseline.
    chmod 4755 "$f"
  fi
done
'

ssh_user 'podman --version'
ssh_user 'podman info --debug >/dev/null'
ssh_user 'set +o pipefail; podman info --debug | head -n 20'

echo "[bootstrap] E) Quadlet directory (must exist; no auto-update log dir pre-creation)"
ssh_root "
install -d -o '$test_user' -g '$test_user' -m 700 '/home/$test_user/.config/containers/systemd'
if [ -e '/home/$test_user/.local/share/podman-auto-update/logs' ]; then
  echo '[remote] ERROR: auto-update log dir already exists (should not be created by bootstrap)' >&2
  exit 21
fi
"

echo "[bootstrap] F) Host port selection for later SSH target container mapping"
port_check="$(
  ssh_root 'command -v ss >/dev/null 2>&1 || echo "[remote] WARNING: ss not found; skipping port check" >&2; ss -ltnp 2>/dev/null | grep -q ":2222" && echo in_use || echo free'
)"
chosen_port="2222"
if [[ "$port_check" == "in_use" ]]; then
  echo "[bootstrap] port 2222 is in use on host; selecting an alternative fixed port"
  for candidate in 2223 22022 20222 30222; do
    if ssh_root "ss -ltnp | grep -q \":$candidate\""; then
      continue
    fi
    chosen_port="$candidate"
    break
  done
fi
echo "[bootstrap] chosen SSH target container host port: $chosen_port"
echo "[bootstrap] export PODUP_E2E_SSH_TARGET_PORT=$chosen_port"
