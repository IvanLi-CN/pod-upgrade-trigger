#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  scripts/e2e/ssh-target/deploy.sh root@<host>

Builds the SSH target Docker image on the remote test host, then (re)creates the
container:
  - container name: podup-test
  - host port: 2222 -> container port: 22
  - SSH user inside container: ivan (key-only auth)
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
image_tag="podup-ssh-target:latest"

ssh_opts=(
  -o BatchMode=yes
  -o StrictHostKeyChecking=accept-new
)

ssh_root() {
  local cmd="$1"
  printf '%s\n' "set -euo pipefail" "$cmd" | ssh "${ssh_opts[@]}" "$root_target" -- bash -s
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

echo "[deploy] root_target=$root_target"
echo "[deploy] host=$host container_name=$container_name image_tag=$image_tag port=${host_port}:22 user=$ops_user"

echo "[deploy] removing any existing container ($container_name)"
ssh_root "
docker rm -f '$container_name' >/dev/null 2>&1 || true
if command -v podman >/dev/null 2>&1; then
  podman rm -f '$container_name' >/dev/null 2>&1 || true
fi
"

echo "[deploy] checking host port ${host_port} availability"
ssh_root "
if command -v ss >/dev/null 2>&1; then
  if ss -ltn 2>/dev/null | awk '{print \$4}' | grep -Eq '(:|\\.)${host_port}\$'; then
    echo \"[remote] ERROR: host port ${host_port} appears in use\" >&2
    exit 1
  fi
else
  echo \"[remote] WARNING: ss not found; skipping port check\" >&2
fi
"

echo "[deploy] creating remote build context"
remote_dir="$(ssh "${ssh_opts[@]}" "$root_target" -- "mktemp -d /tmp/podup-ssh-target.XXXXXX")"
echo "[deploy] remote_dir=$remote_dir"

echo "[deploy] uploading build context (scripts/e2e/ssh-target only)"
tar_no_xattrs=()
if tar --help 2>/dev/null | grep -q -- '--no-xattrs'; then
  tar_no_xattrs+=(--no-xattrs)
fi

COPYFILE_DISABLE=1 COPY_EXTENDED_ATTRIBUTES_DISABLE=1 \
tar -czf - \
  ${tar_no_xattrs[@]+"${tar_no_xattrs[@]}"} \
  --exclude ".DS_Store" \
  --exclude "._*" \
  -C "$script_dir" . \
  | ssh "${ssh_opts[@]}" "$root_target" -- "tar -xzf - -C '$remote_dir'"

echo "[deploy] resolving remote uid/gid for ops user ($ops_user)"
ops_uid="$(ssh_root "id -u '$ops_user'")"
ops_gid="$(ssh_root "id -g '$ops_user'")"
echo "[deploy] ops_uid=$ops_uid ops_gid=$ops_gid"

echo "[deploy] ensuring persistent ssh host key dir exists"
ssh_root "install -d -m 0700 /var/lib/podup-ssh-target/ssh-host-keys"

runtime=""
force_rebuild="${PODUP_SSH_TARGET_REBUILD:-0}"
if [[ "$force_rebuild" != "1" ]]; then
  if ssh_root "docker image inspect '$image_tag' >/dev/null 2>&1"; then
    runtime="docker"
    echo "[deploy] using existing docker image: $image_tag"
  elif ssh_root "command -v podman >/dev/null 2>&1 && podman image exists '$image_tag'"; then
    runtime="podman"
    echo "[deploy] using existing podman image: $image_tag"
  fi
fi

if [[ -z "$runtime" ]]; then
  echo "[deploy] building image (set PODUP_SSH_TARGET_REBUILD=1 to force rebuild)"
  echo "[deploy] docker build (on remote)"
  runtime="docker"
  if ssh_root "
docker version >/dev/null
docker build --pull -t '$image_tag' \
  --build-arg OPS_USER='$ops_user' \
  --build-arg OPS_UID='$ops_uid' \
  --build-arg OPS_GID='$ops_gid' \
  '$remote_dir'
"; then
  runtime="docker"
else
  echo "[deploy] WARNING: docker build failed; falling back to podman (likely nested/containerized host without docker container caps)"
  if ssh_root "
command -v podman >/dev/null
podman build --pull=always -t '$image_tag' \
  --build-arg OPS_USER='$ops_user' \
  --build-arg OPS_UID='$ops_uid' \
  --build-arg OPS_GID='$ops_gid' \
  '$remote_dir'
  "; then
    runtime="podman"
  else
    echo "[deploy] WARNING: podman build failed; falling back to podman run+commit image assembly"
    ssh_root "
command -v podman >/dev/null

build_ctr=\"podup-ssh-target-build\"

# If a previous run was interrupted (e.g. SSH timeout), reuse the most recent
# in-progress build container instead of starting from scratch.
if ! podman inspect \"\$build_ctr\" >/dev/null 2>&1; then
  prev_ctr=\$(podman ps -a --format '{{.Names}}' | grep '^podup-ssh-target-build-' | sort -t- -k5,5n | tail -n 1 || true)
  if [[ -n \"\$prev_ctr\" ]]; then
    podman rename \"\$prev_ctr\" \"\$build_ctr\" >/dev/null 2>&1 || true
  fi
fi

# Best-effort cleanup of older interrupted build containers.
for ctr in \$(podman ps -a --format '{{.Names}}' | grep '^podup-ssh-target-build-' || true); do
  podman rm -f \"\$ctr\" >/dev/null 2>&1 || true
done

if ! podman inspect \"\$build_ctr\" >/dev/null 2>&1; then
  podman run -d --name \"\$build_ctr\" registry.fedoraproject.org/fedora:42 bash -lc 'sleep infinity'
else
  running=\$(podman inspect \"\$build_ctr\" --format '{{.State.Running}}' 2>/dev/null || echo false)
  if [[ \"\$running\" != \"true\" ]]; then
    podman start \"\$build_ctr\" >/dev/null
  fi
fi

if ! podman exec \"\$build_ctr\" bash -lc 'rpm -q systemd dbus openssh-server podman >/dev/null 2>&1'; then
  podman exec \"\$build_ctr\" bash -lc '
set -euo pipefail
for attempt in 1 2 3 4 5; do
  if dnf -y --setopt=install_weak_deps=False --setopt=tsflags=nodocs --setopt=keepcache=True install \
    systemd dbus dbus-tools openssh-server openssh-clients podman podman-docker slirp4netns fuse-overlayfs crun shadow-utils procps-ng iproute which sudo; then
    dnf clean all
    exit 0
  fi
  echo \"[podup-ssh-target] WARNING: dnf install failed (attempt=\${attempt})\" >&2
  sleep 3
done
exit 1
'
fi

podman exec --env OPS_USER='$ops_user' --env OPS_UID='$ops_uid' --env OPS_GID='$ops_gid' \"\$build_ctr\" bash -lc '
set -euo pipefail

if getent group \"\$OPS_GID\" >/dev/null; then
  group_name=\"\$(getent group \"\$OPS_GID\" | cut -d: -f1)\"
else
  groupadd -g \"\$OPS_GID\" \"\$OPS_USER\"
  group_name=\"\$OPS_USER\"
fi

if id -u \"\$OPS_USER\" >/dev/null 2>&1; then
  usermod -u \"\$OPS_UID\" -g \"\$group_name\" -d \"/home/\$OPS_USER\" -m \"\$OPS_USER\"
else
  useradd -m -u \"\$OPS_UID\" -g \"\$group_name\" -s /bin/bash \"\$OPS_USER\"
fi

install -d -m 0700 -o \"\$OPS_UID\" -g \"\$OPS_GID\" \"/home/\$OPS_USER/.ssh\"
install -d -m 0755 -o \"\$OPS_UID\" -g \"\$OPS_GID\" \"/home/\$OPS_USER/.config\"
install -d -m 0755 -o \"\$OPS_UID\" -g \"\$OPS_GID\" \"/home/\$OPS_USER/.config/systemd\"
install -d -m 0755 -o \"\$OPS_UID\" -g \"\$OPS_GID\" \"/home/\$OPS_USER/.config/systemd/user\"
install -d -m 0755 -o \"\$OPS_UID\" -g \"\$OPS_GID\" \"/home/\$OPS_USER/.config/containers\"
install -d -m 0755 -o \"\$OPS_UID\" -g \"\$OPS_GID\" \"/home/\$OPS_USER/.config/containers/systemd\"
install -d -m 0755 -o \"\$OPS_UID\" -g \"\$OPS_GID\" \"/home/\$OPS_USER/.local\"
install -d -m 0755 -o \"\$OPS_UID\" -g \"\$OPS_GID\" \"/home/\$OPS_USER/.local/share\"
install -d -m 0755 -o \"\$OPS_UID\" -g \"\$OPS_GID\" \"/home/\$OPS_USER/.local/share/podup-e2e/quadlets\"

cat >\"/home/\$OPS_USER/.config/containers/storage.conf\" <<\"EOF\"
[storage]
driver = \"vfs\"
EOF
chown \"\$OPS_UID:\$OPS_GID\" \"/home/\$OPS_USER/.config/containers/storage.conf\"
chmod 0644 \"/home/\$OPS_USER/.config/containers/storage.conf\"

install -d -m 0755 \"/var/lib/podup-ssh-target/ssh-host-keys\"
install -d -m 0755 \"/var/lib/systemd/linger\"
: >\"/var/lib/systemd/linger/\$OPS_USER\"

if ! grep -q \"^\$OPS_USER:\" /etc/subuid 2>/dev/null; then echo \"\$OPS_USER:100000:65536\" >>/etc/subuid; fi
if ! grep -q \"^\$OPS_USER:\" /etc/subgid 2>/dev/null; then echo \"\$OPS_USER:100000:65536\" >>/etc/subgid; fi

for f in /usr/bin/newuidmap /usr/bin/newgidmap; do
  if [ -x \"\$f\" ]; then chmod 4755 \"\$f\"; fi
done

mkdir -p /etc/ssh/sshd_config.d
cat >/etc/ssh/sshd_config.d/50-podup-e2e.conf <<\"EOF\"
# pod-upgrade-trigger E2E SSH target
Port 22
Protocol 2

PermitRootLogin no
AllowUsers ivan

PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
PermitEmptyPasswords no

UsePAM yes

# Persist host keys via bind mount to avoid host key churn across redeploys.
HostKey /var/lib/podup-ssh-target/ssh-host-keys/ssh_host_ed25519_key
HostKey /var/lib/podup-ssh-target/ssh-host-keys/ssh_host_rsa_key
EOF

pam_sshd=/etc/pam.d/sshd
if [ -f \"\$pam_sshd\" ] && ! grep -q \"pam_systemd\\\\.so\" \"\$pam_sshd\"; then
  printf \"\\n# Added by pod-upgrade-trigger E2E image (required for systemd --user over SSH)\\nsession    required     pam_systemd.so\\n\" >>\"\$pam_sshd\"
fi

mkdir -p /etc/systemd/system/multi-user.target.wants
ln -sf /usr/lib/systemd/system/sshd.service /etc/systemd/system/multi-user.target.wants/sshd.service
'

podman cp '$remote_dir/entrypoint.sh' \"\$build_ctr:/usr/local/bin/entrypoint.sh\"
podman cp '$remote_dir/podup-e2e-noop.service' \"\$build_ctr:/home/$ops_user/.config/systemd/user/podup-e2e-noop.service\"
podman cp '$remote_dir/podup-e2e-noop.service' \"\$build_ctr:/home/$ops_user/.local/share/podup-e2e/quadlets/podup-e2e-noop.service\"

podman exec --env OPS_USER='$ops_user' --env OPS_UID='$ops_uid' --env OPS_GID='$ops_gid' \"\$build_ctr\" bash -lc '
set -euo pipefail
chmod 0755 /usr/local/bin/entrypoint.sh

home=\"/home/\$OPS_USER\"
mkdir -p \"\$home/.config/systemd/user\"
chown \"\$OPS_UID:\$OPS_GID\" \"\$home/.config/systemd\" \"\$home/.config/systemd/user\"

chown \"\$OPS_UID:\$OPS_GID\" \"/home/\$OPS_USER/.config/systemd/user/podup-e2e-noop.service\"
chmod 0644 \"/home/\$OPS_USER/.config/systemd/user/podup-e2e-noop.service\"

chown \"\$OPS_UID:\$OPS_GID\" \"/home/\$OPS_USER/.local/share/podup-e2e/quadlets/podup-e2e-noop.service\"
chmod 0644 \"/home/\$OPS_USER/.local/share/podup-e2e/quadlets/podup-e2e-noop.service\"
'

podman commit \
  --change 'ENV container=docker' \
  --change 'STOPSIGNAL SIGRTMIN+3' \
  --change 'ENTRYPOINT [\"/usr/local/bin/entrypoint.sh\"]' \
  --change 'CMD [\"/sbin/init\"]' \
  \"\$build_ctr\" '$image_tag'

podman rm -f \"\$build_ctr\" >/dev/null
"
    runtime="podman"
  fi
fi
fi

echo "[deploy] starting container ($container_name)"
if [[ "$runtime" == "docker" ]]; then
  ssh_root "
docker run -d --name '$container_name' \
  --hostname '$container_name' \
  --privileged \
  --cgroupns=host \
  --tmpfs /run \
  --tmpfs /run/lock \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
  -v /var/lib/podup-ssh-target/ssh-host-keys:/var/lib/podup-ssh-target/ssh-host-keys:rw \
  -v /home/$ops_user/.ssh/authorized_keys:/home/$ops_user/.ssh/authorized_keys:ro \
  -p ${host_port}:22 \
  --restart unless-stopped \
  '$image_tag'
"
else
  ssh_root "
podman run -d --name '$container_name' \
  --hostname '$container_name' \
  --privileged \
  --cgroupns=host \
  --tmpfs /run \
  --tmpfs /run/lock \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
  -v /var/lib/podup-ssh-target/ssh-host-keys:/var/lib/podup-ssh-target/ssh-host-keys:rw \
  -v /home/$ops_user/.ssh/authorized_keys:/home/$ops_user/.ssh/authorized_keys:ro \
  -p ${host_port}:22 \
  --restart=unless-stopped \
  '$image_tag'
"
fi

echo "[deploy] ensuring host forwarding allows inbound to published port (iptables)"
ssh_root "
if command -v iptables >/dev/null 2>&1; then
  if iptables -S NETAVARK_FORWARD >/dev/null 2>&1; then
    ip=\$(podman inspect '$container_name' --format '{{.NetworkSettings.Networks.podman.IPAddress}}' 2>/dev/null || true)
    if [[ -n \"\$ip\" ]]; then
      iptables -C NETAVARK_FORWARD -p tcp -d \"\$ip\" --dport 22 -m conntrack --ctstate NEW -j ACCEPT 2>/dev/null \\
        || iptables -I NETAVARK_FORWARD 1 -p tcp -d \"\$ip\" --dport 22 -m conntrack --ctstate NEW -j ACCEPT
    fi
  fi
fi
"

echo "[deploy] cleaning up remote build context"
ssh_root "rm -rf '$remote_dir' || true"

echo "[deploy] waiting for SSH to become reachable"
target_ssh_opts=(
  -o BatchMode=yes
  -o StrictHostKeyChecking=accept-new
  -o ConnectTimeout=5
  -o ConnectionAttempts=1
  -p "$host_port"
)

deadline="$((SECONDS + 45))"
while true; do
  if ssh "${target_ssh_opts[@]}" "${ops_user}@${host}" -- "id" >/dev/null 2>&1; then
    break
  fi
  if [[ "$SECONDS" -ge "$deadline" ]]; then
    echo "[deploy] ERROR: SSH did not become reachable in time" >&2
    if command -v python3 >/dev/null 2>&1; then
      if python3 - "$host" "$host_port" <<'PY'
import socket
import sys

host = sys.argv[1]
port = int(sys.argv[2])
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.settimeout(2.5)
try:
  s.connect((host, port))
  sys.exit(0)
except Exception:
  sys.exit(1)
finally:
  try:
    s.close()
  except Exception:
    pass
PY
      then
        echo "[deploy] TCP connect to ${host}:${host_port} works; likely SSH auth/config issue" >&2
      else
        echo "[deploy] TCP connect to ${host}:${host_port} failed; likely firewall/routing blocks port 2222" >&2
      fi
    fi
    echo "[deploy] HINT: if you see a host key mismatch, run:" >&2
    echo "  ssh-keygen -R \"[${host}]:${host_port}\"" >&2
    exit 1
  fi
  sleep 1
done

echo "[deploy] PASS: ssh reachable: ${ops_user}@${host}:${host_port}"
