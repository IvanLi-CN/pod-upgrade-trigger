# TODO – pod-upgrade-trigger

> Project codename: **pod-upgrade-trigger** — a webhook-driven dispatcher that authenticates events, refreshes Podman images, and restarts the right systemd units on cue.

## Feature Parity With Desired Behavior

- [x] Introduce a first-class scheduler that can periodically trigger `podman-auto-update` (e.g., embed a timer loop or document a companion systemd timer) so the "automatic" requirement works without external glue.
- [x] Expose an HTTP endpoint and CLI flag for triggering *all* units beyond the single `/auto-update` token flow, so Kubernetes/CI/CD integrations can invoke it with richer metadata (caller, reason, dry-run, etc.).
- [x] Generalize the per-service trigger so non-GitHub callers (internal tools, Slack bots) can hit a stable JSON API instead of crafting GitHub payloads, while retaining the lookup-by-service semantics.

## Reliability & Safety

- [x] Persist structured event logs (SQLite `event_log`) for every request to simplify debugging rate-limit or image-mismatch scenarios.
- [x] Add integration tests that mock GitHub payloads, exercise rate limiting, and validate that `systemd-run` invocations are built correctly.
- [x] Provide sample systemd socket/unit files plus `.env` template documenting required environment variables.

## Developer Experience

- [x] Document the state directory layout (`ratelimit.db`, GitHub per-image databases) and add a maintenance command to prune them safely.
- [x] Publish a release process (build, test, package) so the binary in `bin/` can be regenerated reproducibly.

## Future Enhancements

- [x] Optional auto-discovery of webhook-capable systemd units:
  - When enabled via a dedicated flag/env (e.g. `PODUP_AUTO_DISCOVER=1`), scan systemd units by naming convention or explicit marker (such as `X-Webhook-Enabled=yes`) to build the GitHub Webhooks list instead of (or in addition to) `PODUP_MANUAL_UNITS`.
  - Keep the current explicit list as the default/safe behavior; auto-discovery should be opt-in and clearly documented.

## Operational Hardening (2025-11)

- [x] Persist auto-discovered Podman auto-update units (expected ~53) into the state store and expose them via `/api/manual/services`, running discovery only after DB init and logging failures.
- [x] Add startup/self-check paths so `/api/webhooks/status` and `/api/image-locks` return structured errors (not 502) and emit logs when DB or Podman connectivity fails.
- [x] Auto-create/migrate the state DB when missing or unwritable; when impossible, surface actionable guidance on the health page (path + env hints).
- [x] Settings page should show the discovered auto-update unit count plus a summary list, distinct from env-provided manual units, to aid ops reconciliation.

## Task Management Panel – Frontend-First Phase

- [x] Task domain modeling & mock infrastructure (frontend only)
  - Define Task-related TypeScript types in `web` (Task, TaskStatus, TaskKind, TaskUnitSummary, TaskLogEntry, etc.).
  - Add MSW handlers for `/api/tasks` list, `/api/tasks/:id` detail, and actions (`/api/tasks/:id/stop`, `/force-stop`, `/retry`), with rich fake data.
  - Cover core scenarios in mock data: manual/webhook/scheduler/maintenance/other automatic tasks, running/failed/succeeded/cancelled.

- [x] Tasks list page (page mode) UI & interactions (frontend + mock)
  - Introduce `/tasks` route and sidebar navigation entry, wired to `useApi` over mocked endpoints.
  - Implement tasks table with pagination, status/type/unit filters, and unit text search.
  - Implement quick category switcher (e.g. All / Manual / Webhook / Automatic / Maintenance) synced with type filters.
  - Add list polling (e.g. every 5–10 seconds) with proper loading/empty/error states.

- [x] Task detail view & drawer mode (frontend + mock)
  - Implement task detail layout: summary card, per-unit status list, and log timeline backed by mock detail API.
  - Implement right-side drawer component that hosts the full-detail view in a compact layout.
  - Wire `/tasks` list rows to open the drawer on click; support closing without losing list filters/scroll position.
  - Add detail polling for running tasks only, stopping automatically once a terminal state is reached.
  - Implement stop/force-stop/retry flows purely against mocks, updating local state and surfacing toast feedback.

- [x] Integration with existing pages for long-running flows (frontend + mock)
  - For Manual/Maintenance-style actions that may launch long-running work, mock a “create task” API that returns `task_id` and long-running hints.
  - On successful creation, automatically open the corresponding task drawer and start polling its detail.
  - Adjust existing UI flows (where appropriate) to guide users from Events/Webhooks/Maintenance into the Tasks view when they need task-centric insights.

- [x] Backend contract shaping based on frontend experience
  - From the stabilized frontend + mock behavior, extract a concrete Task entity and API contract proposal.
  - Document expected fields, status transitions, and relationships to `event_log` (including how logs are queried per task).
  - Capture any additional fields discovered during UX trials (e.g. richer summaries, counters, or hints for graceful vs force stop).

- [x] Complete MSW mocks for task APIs
  - Implement `/api/tasks` list, `/api/tasks/:id` detail, and `/api/tasks/:id/{stop,force-stop,retry}` handlers with realistic fixture data.
  - Ensure `/tasks` 页面在 mock 模式下有可用的列表数据与抽屉详情，便于前端自测。
