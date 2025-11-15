# TODO – pod-upgrade-trigger

> Project codename: **pod-upgrade-trigger** — a webhook-driven dispatcher that authenticates events, refreshes Podman images, and restarts the right systemd units on cue.

## Feature Parity With Desired Behavior
- [ ] Introduce a first-class scheduler that can periodically trigger `podman-auto-update` (e.g., embed a timer loop or document a companion systemd timer) so the "automatic" requirement works without external glue.
- [ ] Expose an HTTP endpoint and CLI flag for triggering *all* units beyond the single `/auto-update` token flow, so Kubernetes/CI/CD integrations can invoke it with richer metadata (caller, reason, dry-run, etc.).
- [ ] Generalize the per-service trigger so non-GitHub callers (internal tools, Slack bots) can hit a stable JSON API instead of crafting GitHub payloads, while retaining the lookup-by-service semantics.

## Reliability & Safety
- [ ] Persist structured audit logs (JSON lines) for every request to simplify debugging rate-limit or image-mismatch scenarios.
- [ ] Add integration tests that mock GitHub payloads, exercise rate limiting, and validate that `systemd-run` invocations are built correctly.
- [ ] Provide sample systemd socket/unit files plus `.env` template documenting required environment variables.

## Developer Experience
- [ ] Document the state directory layout (`ratelimit.db`, GitHub per-image databases) and add a maintenance command to prune them safely.
- [ ] Publish a release process (build, test, package) so the binary in `bin/` can be regenerated reproducibly.
