Mock binaries for integration testing.

- podman: logs invocations, can fail pull or image prune via env vars.
- systemctl: logs invocations, can fail specific units via env var.
- systemd-run: logs invocations, optional delay/failure, and synchronously executes
  the spawned webhook task for e2e tests.
- Log file: tests/mock-bin/log.txt

Env vars:
- MOCK_PODMAN_FAIL=1         # fail podman pull
- MOCK_PODMAN_PRUNE_FAIL=1   # fail podman image prune -f
- MOCK_SYSTEMCTL_FAIL=unitA,unitB  # fail restart/start for listed units
- MOCK_SYSTEMD_RUN_FAIL=taskA,taskB # fail dispatch for listed systemd-run units
- MOCK_SYSTEMD_RUN_DELAY_MS=250     # sleep before dispatching child (milliseconds)

Usage:
PATH="$(pwd)/tests/mock-bin:$PATH" cargo test
