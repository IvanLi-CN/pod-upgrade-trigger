# 端到端自动化测试方案

## 背景与目标

`webhook-auto-update` 在同一进程中承担 GitHub Webhook 校验、Podman 镜像刷新、systemd 单元重启、手动触发 API、调度器与 SQLite 状态维护等职责。由于大部分副作用可以通过 PATH 注入 mock 二进制以及可配置的 `WEBHOOK_STATE_DIR` 隔离，本方案旨在：

1. 为所有关键路径提供可重复、完全离线的自动化验证能力；
2. 通过 mock 服务记录与断言 Podman/systemctl/systemd-run 调用；
3. 允许测试直接检查 SQLite `event_log`、`rate_limit_tokens`、`image_locks` 等表，验证限流与审计逻辑；
4. 在 CI 中集成 e2e 测试，保证每次提交都覆盖真实调度、重启与错误分支。

## 测试框架架构

### 1. Tokio 异步 e2e 测试套件

- 新建 `tests/e2e.rs`（或拆分多文件）并使用 `#[tokio::test]`。
- 每个用例创建 `TempDir`，设置 `WEBHOOK_STATE_DIR`、`WEBHOOK_DB_URL=sqlite://<tmp>/pod-upgrade-trigger.db`、`WEBHOOK_WEB_DIST=<tmp>/web/dist` 等环境变量。
- 测试前通过 `PATH="$(pwd)/tests/mock-bin:$PATH"` 注入 mock，可并行运行。

### 2. 进程管理器

- 封装 `std::process::Command` 以 server/CLI 模式拉起 `webhook-auto-update`，可捕获 stdout/stderr。
- 通过向子进程 STDIN 写入 HTTP 报文模拟 systemd socket 激活；如需 TCP，可引入测试特性禁用 socket 激活并监听 127.0.0.1。
- 为调度器与 CLI 子命令封装 helper（如 `run_scheduler`, `trigger_units_cli`）。

### 3. Mock 服务矩阵

- 复用 `tests/mock-bin/podman`、`systemctl`，并新增 `systemd-run` mock：记录参数、可配置失败或延迟、必要时触发 `webhook-auto-update run-task`。
- 所有 mock 输出统一写入 `tests/mock-bin/log.txt`（JSON Lines 或 key=value），测试读取后断言调用顺序、参数与失败注入是否生效。
- 通过环境变量控制失败：`MOCK_PODMAN_FAIL`、`MOCK_PODMAN_PRUNE_FAIL`、`MOCK_SYSTEMCTL_FAIL`，新增 `MOCK_SYSTEMD_RUN_FAIL`、`MOCK_SYSTEMD_RUN_DELAY_MS` 等。

### 4. 客户端与数据构造

- 提供 helper 生成 GitHub `package` 与 `registry_package` 事件 JSON，并使用 `GITHUB_WEBHOOK_SECRET` 计算 `x-hub-signature-256`。
- 封装 `/api/manual/trigger`、`/api/manual/services/<name>`、`/auto-update` 的 HTTP 请求构建，支持 dry-run/实跑。
- 对静态资源测试，在 `TempDir/web/dist` 写入伪造的 `index.html`、`assets/` 内容。

### 5. 状态验证工具

- 在测试中用 `sqlx::SqlitePool` 连接临时数据库，断言：
  - `event_log` 中的请求记录（method/path/status/action/meta）；
  - `rate_limit_tokens`、`image_locks` 的插入与清理；
  - `record_system_event` 是否写入调度器与 CLI 事件。
- 对 `last_payload.bin` 等文件做快照，以确认签名失败路径的副作用。

### 6. 失败注入策略

- 通过 mock env 触发 podman/systemctl/systemd-run 失败或慢响应，验证 API/CLI 返回值与重试记录。
- 支持在测试里模拟速率限制饱和（写入 `rate_limit_tokens`），并断言 `prune-state` 后状态恢复。

## 关键端到端场景

1. **GitHub Webhook 正常流程**：发送签名请求 → 通过镜像限流 → `systemd-run` mock 触发 `run-task` → `podman pull` 与 `systemctl restart` 按顺序记入日志 → `event_log` 存储对应行。
2. **速率限制与清理**：预填 `rate_limit_tokens`，短时间重复触发得到 HTTP 429，执行 `webhook-auto-update prune-state --max-age-hours 48` 后重试成功。
3. **手动触发 API**：`POST /api/manual/trigger`（`all=true`、`dry_run` 与正常模式）以及 `/api/manual/services/<slug>`（携带 `image/caller/reason`）；断言 dry-run 不产生 systemctl 调用、审计字段正确写入。
4. **调度器循环**：`webhook-auto-update scheduler --interval 1 --max-iterations 2`；验证 mock 里 `podman-auto-update.service` 的调用和 `record_system_event` 记录。
5. **错误路径**：设置 `MOCK_PODMAN_FAIL=1` 或 `MOCK_SYSTEMD_RUN_FAIL=unitA`；确认 HTTP/CLI 返回值、SQLite 中的失败事件，以及 `last_payload.bin` dump。
6. **静态资源与健康检查**：`GET /health` 正常返回；`WEBHOOK_WEB_DIST` 存在时 `GET /`、`/assets/*` 提供对应文件。
7. **维护命令**：执行 `trigger-units`、`trigger-all --dry-run`、`prune-state` 等 CLI，查证 mock 日志与数据库状态。

## 执行与 CI 集成

- 新增 `just test-e2e`（或 `make test-e2e`）目标：`PATH="$PWD/tests/mock-bin:$PATH" cargo test --test e2e -- --nocapture`。
- 若运行时间较长，可使用 feature gate（如 `--features e2e-full`）在本地/CI 选择性开启。
- CI Workflow：
  1. 安装依赖（Rust stable + sqlx 所需的 SQLite）。
  2. 运行 `cargo test`（快速单测）。
  3. 设置 `PATH` 注入 mock，执行 `just test-e2e`；失败时上传 `tests/mock-bin/log.txt`、子进程 stdout/stderr、e2e 临时 SQLite DB 作为 artifacts。
  4. 成功后可继续 `cargo build --release` 以保持原有发布流程。

## 推进计划

1. 落地 `tests/e2e.rs` 框架与 helper（进程管理、HTTP 发包、SQLite 断言、mock 日志解析）。
2. 扩展 `tests/mock-bin`：实现 `systemd-run` mock，规范日志格式，并提供清理脚本。
3. 补充 `docs/testing.md`（或在 README 对应章节）说明本方案的运行方式、环境变量、常见问题。
4. 在 CI PR workflow 中加入 `test-e2e`，确保提交默认跑完整自动化用例。

以上设计可确保在完全离线、可控的 mock 环境中验证 Podman/systemd 的复杂链路，并通过 SQLite 与日志断言保证业务语义正确。EOF
