#!/usr/bin/env bash
set -euo pipefail

host_keys_dir="/var/lib/podup-ssh-target/ssh-host-keys"

mkdir -p /run/sshd "$host_keys_dir"

if [[ ! -f "${host_keys_dir}/ssh_host_ed25519_key" ]]; then
  ssh-keygen -t ed25519 -f "${host_keys_dir}/ssh_host_ed25519_key" -N "" >/dev/null
fi

if [[ ! -f "${host_keys_dir}/ssh_host_rsa_key" ]]; then
  ssh-keygen -t rsa -b 4096 -f "${host_keys_dir}/ssh_host_rsa_key" -N "" >/dev/null
fi

chmod 600 "${host_keys_dir}/ssh_host_ed25519_key" "${host_keys_dir}/ssh_host_rsa_key"
chmod 644 "${host_keys_dir}/ssh_host_ed25519_key.pub" "${host_keys_dir}/ssh_host_rsa_key.pub"

exec "$@"

