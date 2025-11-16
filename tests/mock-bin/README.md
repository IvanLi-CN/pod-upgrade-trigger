Mock binaries for integration testing.

- podman: logs invocations, can fail pull or image prune via env vars.
- systemctl: logs invocations, can fail specific units via env var.
- Log file: tests/mock-bin/log.txt

Env vars:
- MOCK_PODMAN_FAIL=1         # fail podman pull
- MOCK_PODMAN_PRUNE_FAIL=1   # fail podman image prune -f
- MOCK_SYSTEMCTL_FAIL=unitA,unitB  # fail restart/start for listed units

Usage:
PATH="$(pwd)/tests/mock-bin:$PATH" cargo test
