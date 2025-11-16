# pod-upgrade-trigger

`webhook-auto-update` is a socket-activated webhook dispatcher that validates GitHub
package events, enforces rate limits, refreshes Podman images, and restarts the
correct systemd units. The binary also ships a JSON API for manual triggers,
a scheduler loop for `podman-auto-update`, and utilities for maintaining the
state directory.

## State Directory Layout

The service stores durable data under `WEBHOOK_STATE_DIR` (defaults to
`/srv/pod-upgrade-trigger`). The important files and sub-directories are:

| Path | Purpose |
| --- | --- |
| `ratelimit.db` / `ratelimit.lock` | Sliding-window timestamp log used by the global `/auto-update` throttler |
| `github-image-limits/*.db` | Per-image timestamp logs that prevent spamming `podman pull` for the same image |
| `github-image-locks/*.lock` | Lock files that ensure only one redeploy runs per image at a time |
| `last_payload.bin` | Dump of the last signature-mismatched payload for debugging |
| `web/dist` | Optional static assets served on `/` when the web UI bundle is deployed |
| SQLite `event_log` | 所有触发/调度请求都会同步写入 SQLite（默认 `data/pod-upgrade-trigger.db`） |

Run the daemon via systemd (see `systemd/webhook-auto-update.service`, which now
executes `webhook-auto-update server`). For housekeeping, use the CLI
subcommands below; for example:

```bash
webhook-auto-update prune-state --max-age-hours 48      # remove entries older than 48h
webhook-auto-update prune-state --dry-run               # show what would be deleted
```

This command prunes stale timestamps, drops empty `.db` files, and removes lock
files whose `mtime` is older than the retention window.

### 结构化事件记录

- 程序默认连接 `sqlite://data/pod-upgrade-trigger.db`，自动创建目录并运行
  `migrations/` 内的脚本初始化 `event_log` 表。若要自定义位置，可设置
  `WEBHOOK_DB_URL` 覆盖连接串。
- 所有 HTTP 请求、CLI 手动触发与调度器 tick 都会异步插入数据库，字段包含
  `request_id/method/path/status/action/meta` 等，可用于报表、运营统计或问题定位。

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
