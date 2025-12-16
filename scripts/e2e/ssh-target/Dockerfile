FROM registry.fedoraproject.org/fedora:42

ARG OPS_USER=ivan
ARG OPS_UID=1000
ARG OPS_GID=1000

ENV container=docker

STOPSIGNAL SIGRTMIN+3

RUN set -euo pipefail; \
  dnf -y update; \
  dnf -y install \
    systemd \
    dbus \
    dbus-tools \
    openssh-server \
    openssh-clients \
    podman \
    podman-docker \
    slirp4netns \
    fuse-overlayfs \
    crun \
    shadow-utils \
    procps-ng \
    iproute \
    which \
    sudo; \
  dnf clean all

RUN set -euo pipefail; \
  if getent group "${OPS_GID}" >/dev/null; then \
    group_name="$(getent group "${OPS_GID}" | cut -d: -f1)"; \
  else \
    groupadd -g "${OPS_GID}" "${OPS_USER}"; \
    group_name="${OPS_USER}"; \
  fi; \
  if id -u "${OPS_USER}" >/dev/null 2>&1; then \
    usermod -u "${OPS_UID}" -g "${group_name}" -d "/home/${OPS_USER}" -m "${OPS_USER}"; \
  else \
    useradd -m -u "${OPS_UID}" -g "${group_name}" -s /bin/bash "${OPS_USER}"; \
  fi; \
  install -d -m 0700 -o "${OPS_UID}" -g "${OPS_GID}" "/home/${OPS_USER}/.ssh"; \
  install -d -m 0755 -o "${OPS_UID}" -g "${OPS_GID}" "/home/${OPS_USER}/.config"; \
  install -d -m 0755 -o "${OPS_UID}" -g "${OPS_GID}" "/home/${OPS_USER}/.config/containers"; \
  install -d -m 0755 -o "${OPS_UID}" -g "${OPS_GID}" "/home/${OPS_USER}/.config/containers/systemd"; \
  install -d -m 0755 -o "${OPS_UID}" -g "${OPS_GID}" "/home/${OPS_USER}/.local"; \
  install -d -m 0755 -o "${OPS_UID}" -g "${OPS_GID}" "/home/${OPS_USER}/.local/share"; \
  install -d -m 0755 -o "${OPS_UID}" -g "${OPS_GID}" "/home/${OPS_USER}/.local/share/podup-e2e/quadlets"; \
  install -d -m 0755 "/var/lib/podup-ssh-target/ssh-host-keys"; \
  install -d -m 0755 "/var/lib/systemd/linger"; \
  : >"/var/lib/systemd/linger/${OPS_USER}"; \
  if ! grep -q "^${OPS_USER}:" /etc/subuid 2>/dev/null; then echo "${OPS_USER}:100000:65536" >>/etc/subuid; fi; \
  if ! grep -q "^${OPS_USER}:" /etc/subgid 2>/dev/null; then echo "${OPS_USER}:100000:65536" >>/etc/subgid; fi; \
  for f in /usr/bin/newuidmap /usr/bin/newgidmap; do \
    if [ -x "$f" ]; then chmod 4755 "$f"; fi; \
  done

RUN set -euo pipefail; \
  mkdir -p /etc/ssh/sshd_config.d; \
  cat >/etc/ssh/sshd_config.d/50-podup-e2e.conf <<'EOF'
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

RUN set -euo pipefail; \
  pam_sshd=/etc/pam.d/sshd; \
  if [ -f "$pam_sshd" ] && ! grep -q "pam_systemd\\.so" "$pam_sshd"; then \
    printf "\n# Added by pod-upgrade-trigger E2E image (required for systemd --user over SSH)\nsession    required     pam_systemd.so\n" >>"$pam_sshd"; \
  fi

RUN set -euo pipefail; \
  mkdir -p /etc/systemd/system/multi-user.target.wants; \
  ln -sf /usr/lib/systemd/system/sshd.service /etc/systemd/system/multi-user.target.wants/sshd.service

COPY entrypoint.sh /usr/local/bin/entrypoint.sh
COPY podup-e2e-noop.service /usr/local/share/podup-e2e-noop.service

RUN set -euo pipefail; \
  chmod 0755 /usr/local/bin/entrypoint.sh; \
  ops_uid="${OPS_UID}"; \
  ops_gid="${OPS_GID}"; \
  install -d -m 0755 -o "${ops_uid}" -g "${ops_gid}" "/home/${OPS_USER}/.config/systemd/user"; \
  install -o "${ops_uid}" -g "${ops_gid}" -m 0644 /usr/local/share/podup-e2e-noop.service "/home/${OPS_USER}/.config/systemd/user/podup-e2e-noop.service"; \
  install -o "${ops_uid}" -g "${ops_gid}" -m 0644 /usr/local/share/podup-e2e-noop.service "/home/${OPS_USER}/.local/share/podup-e2e/quadlets/podup-e2e-noop.service"

RUN set -euo pipefail; \
  printf '%s\n' '[storage]' 'driver = "vfs"' >"/home/${OPS_USER}/.config/containers/storage.conf"; \
  chown "${OPS_UID}:${OPS_GID}" "/home/${OPS_USER}/.config/containers/storage.conf"; \
  chmod 0644 "/home/${OPS_USER}/.config/containers/storage.conf"

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["/sbin/init"]
