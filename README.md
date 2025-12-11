# pod-upgrade-trigger

`pod-upgrade-trigger` is a small HTTP service that validates GitHub package events,
enforces rate limits, refreshes Podman images, and restarts the correct systemd
units. The binary also ships a JSON API for manual triggers, a scheduler loop for
`podman-auto-update`, and utilities for maintaining the state directory. It is
designed to run as a normal HTTP server via the `http-server` subcommand.

## State Directory Layout

The service stores durable data under `PODUP_STATE_DIR` (defaults to
`/srv/pod-upgrade-trigger`). The important files and sub-directories are:

| Path | Purpose |
| --- | --- |
| `data/pod-upgrade-trigger.db` | SQLite 数据库，包含请求事件、限流计数与镜像锁 |
| `last_payload.bin` | Dump of the last signature-mismatched payload for debugging |
| `web/dist` | Optional static assets served on `/` when present on disk; overrides the embedded Web UI bundle |

Run the daemon as a normal HTTP service for most deployments. The recommended
unit is `pod-upgrade-trigger http-server`, which listens on `PODUP_HTTP_ADDR`
(`0.0.0.0:25111` by default when not overridden). Older socket-activation units
have been removed; the only supported entry point is the `http-server` subcommand.

For housekeeping, use the CLI subcommands below; for example:

```bash
PATH="$PWD/tests/mock-bin:$PATH" pod-upgrade-trigger trigger-units demo.service --dry-run
PATH="$PWD/tests/mock-bin:$PATH" pod-upgrade-trigger prune-state --max-age-hours 48
```

The `prune-state` command deletes stale rate-limit rows and aged image locks
from the SQLite database (and also removes any leftover legacy files from older
versions).

## Local HTTP server + Web UI

To try the built-in web UI locally:

1. Build the Rust binary:
   ```bash
   cargo build --bin pod-upgrade-trigger
   ```
2. Build the frontend bundle:
   ```bash
   cd web
   npm install
   npm run build
   cd ..
   ```
3. Start the HTTP server with a local state dir and dev-friendly auth:
   ```bash
   PODUP_STATE_DIR="$PWD" \
   PODUP_TOKEN="dev-token" \
   PODUP_MANUAL_TOKEN="dev-token" \
   PODUP_DEV_OPEN_ADMIN="1" \
   PODUP_HTTP_ADDR="127.0.0.1:25111" \
   target/debug/pod-upgrade-trigger http-server
   ```

Then open `http://127.0.0.1:25111/` in your browser. In the top status bar,
enter `dev-token` as the manual token to access admin-only APIs via the UI.
The binary automatically serves UI assets in this order: `${PODUP_STATE_DIR}/web/dist` → `$CWD/web/dist` → the embedded bundle packaged in the release binary. No Web UI override environment variable is supported. Routes like `/`, `/events`, `/tasks`, and `/settings` will render from whichever source is found first; removing the on-disk bundle falls back to the embedded UI.

### Release build with embedded Web UI

Release artifacts embed the frontend bundle so host systemd deployments only need the binary. Build steps (example):

```bash
cd web
bun install --frozen-lockfile || bun install
bun run build
cd ..
cargo build --release --bin pod-upgrade-trigger
```

`build.rs` checks for `web/dist/index.html` during `cargo build --release` and will fail the build with a clear message if the bundle is missing.

Host systemd deployments only need the release binary plus env files; `/` will render the embedded UI unless a disk `web/dist` is present.

## ForwardAuth and dev mode

The service can optionally protect admin-only APIs (Events, Manual, Webhooks)
using a ForwardAuth-style header:

- In production:
  - Set `FORWARD_AUTH_HEADER`, e.g. `X-Forwarded-User`;
  - Set `FORWARD_AUTH_ADMIN_VALUE` to the value that identifies an admin user;
  - Optionally configure `FORWARD_AUTH_NICKNAME_HEADER` and `ADMIN_MODE_NAME`.
  - Do **not** set `DEV_OPEN_ADMIN`.
- In development:
  - Either leave `FORWARD_AUTH_HEADER` / `FORWARD_AUTH_ADMIN_VALUE` unset, **or**
  - Set `DEV_OPEN_ADMIN=1` to completely bypass ForwardAuth checks and treat all
    requests as admin.

If `FORWARD_AUTH_HEADER` and `FORWARD_AUTH_ADMIN_VALUE` are set but `DEV_OPEN_ADMIN`
is not, missing/incorrect auth headers will cause `401 Unauthorized` on admin APIs
and the UI will route to `/401`.

### 结构化事件记录

- 程序默认连接 `sqlite://data/pod-upgrade-trigger.db`，自动创建目录并运行
  `migrations/` 内的脚本初始化事件表与限流表。若要自定义位置，可设置
  `PODUP_DB_URL` 覆盖连接串。
- 所有 HTTP 请求、CLI 手动触发与调度器 tick 都会异步插入 `event_log` 表，字段包含
  `request_id/method/path/status/action/meta` 等，可用于报表、运营统计或问题定位。
- 速率限制计数与镜像锁也存放在同一个 SQLite 数据库中，无需额外文件。

## Scheduler and Manual Triggers

- `pod-upgrade-trigger scheduler --interval 600` runs the auto-update unit
  every ten minutes. Optional `--max-iterations` allows bounded runs for testing.
- `pod-upgrade-trigger trigger-units service-a service-b --caller ci --reason deploy`
  restarts the listed services immediately.
- `pod-upgrade-trigger trigger-all --dry-run` shows which units would be touched
  without contacting systemd.
- HTTP callers can use `POST /api/manual/trigger` with a JSON payload:
  ```json
  {
    "token": "...",
    "all": true,
    "dry_run": false,
    "caller": "ci",
    "reason": "nightly"
  }
  ```
- Service-specific redeploys live under `/api/manual/services/<name>` and accept
  optional `image`, `caller`, and `reason` fields.

## Release Process

1. Run the full test suite: `cargo test`.
2. Build a release binary: `cargo build --release`.
3. Copy the binary into `bin/pod-upgrade-trigger` for distribution or package it
   via your preferred artifact system.
4. Regenerate the systemd units as needed (`systemd/pod-upgrade-trigger.*`) and
   verify the `.env` template before deploying.
5. 同步 `migrations/` 目录，确保线上 SQLite schema 随版本演进；将 `data/` 中的
   数据库文件纳入备份策略。
