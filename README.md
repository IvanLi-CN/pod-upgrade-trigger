# pod-upgrade-trigger

`webhook-auto-update` is a small HTTP service that validates GitHub package events,
enforces rate limits, refreshes Podman images, and restarts the correct systemd
units. The binary also ships a JSON API for manual triggers, a scheduler loop for
`podman-auto-update`, and utilities for maintaining the state directory. It can be
run as a normal HTTP server (`http-server` 子命令)，也可以在需要时通过 systemd socket 兼容旧部署。

## State Directory Layout

The service stores durable data under `WEBHOOK_STATE_DIR` (defaults to
`/srv/pod-upgrade-trigger`). The important files and sub-directories are:

| Path | Purpose |
| --- | --- |
| `data/pod-upgrade-trigger.db` | SQLite 数据库，包含请求事件、限流计数与镜像锁 |
| `last_payload.bin` | Dump of the last signature-mismatched payload for debugging |
| `web/dist` | Optional static assets served on `/` when the web UI bundle is deployed |

Run the daemon as a normal HTTP service for most deployments. The recommended
unit is `webhook-auto-update http-server`, which listens on `WEBHOOK_HTTP_ADDR`
(`0.0.0.0:25111` by default when not overridden). A legacy systemd socket unit
is still shipped for backward compatibility but should not be used for new setups.

For housekeeping, use the CLI subcommands below; for example:

```bash
PATH="$PWD/tests/mock-bin:$PATH" webhook-auto-update trigger-units demo.service --dry-run
PATH="$PWD/tests/mock-bin:$PATH" webhook-auto-update prune-state --max-age-hours 48
```

The `prune-state` command deletes stale rate-limit rows and aged image locks
from the SQLite database (and also removes any leftover legacy files from older
versions).

## Local HTTP server + Web UI

To try the built-in web UI locally:

1. Build the Rust binary:
   ```bash
   cargo build --bin webhook-auto-update
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
   WEBHOOK_STATE_DIR="$PWD" \
   WEBHOOK_WEB_DIST="$PWD/web/dist" \
   WEBHOOK_TOKEN="dev-token" \
   WEBHOOK_MANUAL_TOKEN="dev-token" \
   DEV_OPEN_ADMIN="1" \
   WEBHOOK_HTTP_ADDR="127.0.0.1:25111" \
   target/debug/webhook-auto-update http-server
   ```

Then open `http://127.0.0.1:25111/` in your browser. In the top status bar,
enter `dev-token` as the manual token to access admin-only APIs via the UI.

### 结构化事件记录

- 程序默认连接 `sqlite://data/pod-upgrade-trigger.db`，自动创建目录并运行
  `migrations/` 内的脚本初始化事件表与限流表。若要自定义位置，可设置
  `WEBHOOK_DB_URL` 覆盖连接串。
- 所有 HTTP 请求、CLI 手动触发与调度器 tick 都会异步插入 `event_log` 表，字段包含
  `request_id/method/path/status/action/meta` 等，可用于报表、运营统计或问题定位。
- 速率限制计数与镜像锁也存放在同一个 SQLite 数据库中，无需额外文件。

## Scheduler and Manual Triggers

- `webhook-auto-update scheduler --interval 600` runs the auto-update unit
  every ten minutes. Optional `--max-iterations` allows bounded runs for testing.
- `webhook-auto-update trigger-units service-a service-b --caller ci --reason deploy`
  restarts the listed services immediately.
- `webhook-auto-update trigger-all --dry-run` shows which units would be touched
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
3. Copy the binary into `bin/webhook-auto-update` for distribution or package it
   via your preferred artifact system.
4. Regenerate the systemd units as needed (`systemd/webhook-auto-update.*`) and
   verify the `.env` template before deploying.
5. 同步 `migrations/` 目录，确保线上 SQLite schema 随版本演进；将 `data/` 中的
   数据库文件纳入备份策略。
