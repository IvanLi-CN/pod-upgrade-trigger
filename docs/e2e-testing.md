# 端到端自动化测试方案

## 背景与目标

`pod-upgrade-trigger` 在同一进程中承担 GitHub Webhook 校验、Podman 镜像刷新、systemd 单元重启、手动触发 API、调度器与 SQLite 状态维护等职责。由于大部分副作用可以通过 PATH 注入 mock 二进制以及可配置的 `PODUP_STATE_DIR` 隔离，本方案旨在：

1. 为所有关键路径提供可重复、完全离线的自动化验证能力；
2. 通过 mock 服务记录与断言 Podman/systemctl/systemd-run 调用；
3. 允许测试直接检查 SQLite `event_log`、`rate_limit_tokens`、`image_locks` 等表，验证限流与审计逻辑；
4. 在 CI 中集成 e2e 测试，保证每次提交都覆盖真实调度、重启与错误分支。

## 测试框架架构

### 1. Tokio 异步 e2e 测试套件

- 新建 `tests/e2e.rs`（或拆分多文件）并使用 `#[tokio::test]`。
- 每个用例创建 `TempDir`，设置 `PODUP_STATE_DIR`、`PODUP_DB_URL=sqlite://<tmp>/pod-upgrade-trigger.db` 等环境变量。
- 测试前通过 `PATH="$(pwd)/tests/mock-bin:$PATH"` 注入 mock，可并行运行。

### 2. 进程管理器

- 封装 `std::process::Command` 以 server/CLI 模式拉起 `pod-upgrade-trigger`，可捕获 stdout/stderr。
- 通过向子进程 STDIN 写入 HTTP 报文驱动单次 `server` 子命令，或在需要时使用 `http-server` 子命令监听 `127.0.0.1` 并通过 TCP 发送请求。
- 为调度器与 CLI 子命令封装 helper（如 `run_scheduler`, `trigger_units_cli`）。

### 3. Mock 服务矩阵

- 复用 `tests/mock-bin/podman`、`systemctl`，并新增 `systemd-run` mock：记录参数、可配置失败或延迟、必要时触发 `pod-upgrade-trigger run-task`。
- 所有 mock 输出统一写入 `tests/mock-bin/log.txt`（JSON Lines 或 key=value），测试读取后断言调用顺序、参数与失败注入是否生效。
- 通过环境变量控制失败：`MOCK_PODMAN_FAIL`、`MOCK_PODMAN_PRUNE_FAIL`、`MOCK_SYSTEMCTL_FAIL`，新增 `MOCK_SYSTEMD_RUN_FAIL`、`MOCK_SYSTEMD_RUN_DELAY_MS` 等。

### 4. 客户端与数据构造

- 提供 helper 生成 GitHub `package` 与 `registry_package` 事件 JSON，并使用 `PODUP_GH_WEBHOOK_SECRET` 计算 `x-hub-signature-256`。
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
2. **速率限制与清理**：预填 `rate_limit_tokens`，短时间重复触发得到 HTTP 429，执行 `pod-upgrade-trigger prune-state --max-age-hours 48` 后重试成功。
3. **手动触发 API**：`POST /api/manual/trigger`（`all=true`、`dry_run` 与正常模式）以及 `/api/manual/services/<slug>`（携带 `image/caller/reason`）；断言 dry-run 不产生 systemctl 调用、审计字段正确写入。
4. **调度器循环**：`pod-upgrade-trigger scheduler --interval 1 --max-iterations 2`；验证 mock 里 `podman-auto-update.service` 的调用和 `record_system_event` 记录。
5. **错误路径**：设置 `MOCK_PODMAN_FAIL=1` 或 `MOCK_SYSTEMD_RUN_FAIL=unitA`；确认 HTTP/CLI 返回值、SQLite 中的失败事件，以及 `last_payload.bin` dump。
6. **静态资源与健康检查**：`GET /health` 正常返回；`PODUP_STATE_DIR/web/dist` 或内置 `/srv/app/web` 存在时 `GET /`、`/assets/*` 提供对应文件。
7. **维护命令**：执行 `trigger-units`、`trigger-all --dry-run`、`prune-state` 等 CLI，查证 mock 日志与数据库状态。
   - 对非 `--dry-run` 的 CLI 触发命令，额外验证：
     - `tasks` / `task_units` 中有对应记录（`source = \"cli\"`）；
     - `event_log` 中的 `cli-trigger` / `cli-prune-state` 事件包含相同的 `task_id` 元数据。

## 执行与 CI 集成

- 新增 `scripts/test-e2e.sh`：自动注入 `PATH="$PWD/tests/mock-bin:$PATH"` 并运行 `cargo test --test e2e -- --nocapture`，本地执行 `./scripts/test-e2e.sh` 即可。
- 若运行时间较长，可使用 feature gate（如 `--features e2e-full`）在本地/CI 选择性开启。
- CI Workflow：
  1. 安装依赖（Rust stable + sqlx 所需的 SQLite）。
  2. 运行 `cargo test`（快速单测）。
  3. 通过 `./scripts/test-e2e.sh` 执行全量 e2e；失败时上传 `tests/mock-bin/log.txt`、子进程 stdout/stderr、e2e 临时 SQLite DB 作为 artifacts。
  4. 成功后可继续 `cargo build --release` 以保持原有发布流程。

## 推进计划

1. 落地 `tests/e2e.rs` 框架与 helper（进程管理、HTTP 发包、SQLite 断言、mock 日志解析）。
2. 扩展 `tests/mock-bin`：实现 `systemd-run` mock，规范日志格式，并提供清理脚本。
3. 补充 `docs/testing.md`（或在 README 对应章节）说明本方案的运行方式、环境变量、常见问题。
4. 在 CI PR workflow 中加入 `test-e2e`，确保提交默认跑完整自动化用例。

以上设计可确保在完全离线、可控的 mock 环境中验证 Podman/systemd 的复杂链路，并通过 SQLite 与日志断言保证业务语义正确。

---

## Web UI 自动化 E2E 测试设计

> 本节聚焦前端 React/Vite Web UI，在真实 `http-server` + mock systemd/podman 环境下，通过 Playwright 驱动浏览器完成端到端验证。后续开发 UI E2E 用例时，应以本节作为完成标准。

### 1. 目标与约束

**目标**

- 在本地与 CI 中一键运行 Web UI E2E：
- 启动真实后端 `pod-upgrade-trigger http-server`（完全 mock 副作用）；
  - 使用无头浏览器驱动 UI，覆盖核心功能与典型错误路径；
  - 从浏览器视角验证页面渲染、交互、导航与 API 行为。

**约束**

- 不接入真实 systemd/podman，所有命令均走 `tests/mock-bin`。
- 不依赖真实 ForwardAuth，测试环境统一使用 `DEV_OPEN_ADMIN=1` 或无需配置 ForwardAuth。
- 每次测试使用独立 `PODUP_STATE_DIR`/SQLite 文件，保证结果可重复、无共享状态污染。

### 2. 技术选型与总体框架

**工具选型**

- 前端测试框架：Playwright + TypeScript。
- 目录约定：
  - `web/playwright.config.ts`：Playwright 配置（`baseURL`, `viewport`, `reporter` 等）。
  - `web/tests/ui/*.spec.ts`：UI E2E 测试用例。
- 统一入口脚本：
  - `scripts/test-ui-e2e.sh`：
    1. 构建后端：`cargo build --bin pod-upgrade-trigger`
    2. 构建前端：`cd web && npm install && npm run build`
    3. 创建临时 state 目录（如 `target/ui-e2e/state-XXXX`）
    4. 以 mock 环境启动 `http-server`
    5. 在 `web/` 下运行 `npx playwright test`

**http-server 启动环境（示例）**

```bash
PODUP_STATE_DIR="$STATE_DIR" \
PODUP_DB_URL="sqlite://$STATE_DIR/pod-upgrade-trigger.db" \
PODUP_TOKEN="e2e-token" \
PODUP_MANUAL_TOKEN="e2e-token" \
PODUP_GH_WEBHOOK_SECRET="e2e-secret" \
PODUP_MANUAL_UNITS="svc-alpha.service,svc-beta.service" \
PODUP_DEV_OPEN_ADMIN="1" \
PODUP_HTTP_ADDR="127.0.0.1:25211" \
PODUP_PUBLIC_BASE_URL="http://127.0.0.1:25211" \
PODUP_AUDIT_SYNC="1" \
PATH="$REPO/tests/mock-bin:$PATH" \
nohup target/debug/pod-upgrade-trigger http-server >ui-e2e-http.log 2>&1 &
```

Playwright 配置中的 `baseURL` 对应 `http://127.0.0.1:25211`。

### 3. Playwright 项目结构

- `web/playwright.config.ts`
  - `use.baseURL = process.env.UI_E2E_BASE_URL || 'http://127.0.0.1:25211'`
  - `use.viewport = { width: 1280, height: 720 }`
  - CI 下 `retries = 1`；`reporter = ['list', ['html', { outputFolder: 'playwright-report' }]]`
- 测试文件（建议）：
  - `web/tests/ui/smoke.spec.ts`
  - `web/tests/ui/manual.spec.ts`
  - `web/tests/ui/webhooks.spec.ts`
  - `web/tests/ui/events.spec.ts`
  - `web/tests/ui/maintenance.spec.ts`
  - `web/tests/ui/settings.spec.ts`
  - `web/tests/ui/auth.spec.ts`（可选，用于 ForwardAuth 严格模式）

### 4. 测试用例分层设计

以下多级列表是 UI E2E 的目标用例集。开发完成时，应至少覆盖到这些场景。

#### A. 全局 & 路由基础（smoke.spec.ts）

1. **首页加载与导航框架**
   - 打开 `/`：
     - 顶部显示标题 “Webhook Control”；
     - Health badge 初始为 `Healthy`（`/health` 200）；
     - Scheduler 显示 interval（如 `900s`）与 tick（无事件时允许为 `--`）；
     - SSE badge 在 `/sse/hello` 成功后显示 `SSE ok`。
   - 左侧导航：
     - Dashboard / Manual / Webhooks / Events / Maintenance / Settings 均存在；
     - 点击每个入口，高亮状态与 URL 路径变化正确（例如 `/manual`、`/webhooks`）。

2. **SPA 路由兜底**
   - 直接访问 `/manual`、`/webhooks`、`/events`、`/maintenance`、`/settings`：
     - 服务端返回 index.html，前端路由渲染对应页面；
     - 页面不显示原始 “not found”，而是正确的模块 UI。

3. **401 页面行为（严格模式下）**
   - 在单独配置下启动后端（配置 ForwardAuth，关闭 `DEV_OPEN_ADMIN`）：
     - 未带头访问 `/settings`：
       - `/api/settings` 返回 401；
       - 前端跳转 `/401`，显示 “未授权 · 401” 提示与当前请求路径；
       - 通过 `history.replaceState`，地址栏保持 `/settings`，刷新后仍停留在 Settings 路由。

#### B. Manual 手动触发控制台（manual.spec.ts）

1. **服务列表加载**
   - 访问 `/manual`：
     - 标题 “触发全部单元”、“按单元触发”、“历史记录”；
     - 从 `/api/manual/services` 返回的 `svc-alpha.service`、`svc-beta.service` 在列表中展示：
       - 每行包含 `display_name`、`unit`、image/caller/reason 输入框；
       - Dry 开关与 “触发” 按钮；
       - GitHub 路径 badge：`/github-package-update/<slug>`。

2. **触发全部（dry-run）**
   - 将 Dry run 打开，填写 Caller/Reason，点击 “触发全部”：
     - 发送 `POST /api/manual/trigger`，body 包含 `all:true`、`dry_run:true`、caller/reason；
     - 返回 202/207 时：
       - 顶部出现成功或部分失败 Toast；
       - “历史记录” 增加一条 `trigger-all (N)` 记录，时间接近当前；
       - 点击历史记录项跳转 `/events?request_id=...`。

3. **按单元触发**
   - 对某个 unit（如 `svc-alpha.service`）：
     - Dry=false，填写 image/caller/reason，点击 “触发”：
       - 请求 `POST /api/manual/services/svc-alpha`，body 包含字段；
       - 返回 status 为 `triggered` 或 `dry-run`；
       - 弹出对应 Toast，历史记录增加 `trigger-unit svc-alpha.service`。

4. **错误处理**
   - 通过 env/后端控制使 `POST /api/manual/trigger` 返回 500：
     - UI 显示错误 Toast；
     - 历史记录不新增成功条目；
     - 页面不崩溃，仍可再次尝试触发。

#### C. Webhooks GitHub 面板（webhooks.spec.ts）

1. **配置加载与禁用态**
   - 首次进入 `/webhooks` 时：
     - `/api/config`、`/api/webhooks/status` 尚未返回时，页面显示 “Loading config and webhook status…”；
     - “复制 URL” / “查看事件” 按钮为 disabled；
     - 不渲染误导性的 URL 文本。
   - 等待加载完成：
     - 隐藏 loading 文案；
     - 按钮恢复为可用。

2. **多 unit 展示**
   - 配置 `PODUP_MANUAL_UNITS=svc-alpha.service,svc-beta.service`：
     - Webhooks 页面显示两个单元卡片：`svc-alpha.service`、`svc-beta.service`；
     - 卡片内包含：
       - unit 名、slug、HMAC 状态；
       - Webhook URL / Redeploy URL badge；
       - 预期镜像（若配置）。

3. **完整 URL 显示与复制**
   - 设置 `PODUP_PUBLIC_BASE_URL=https://example.com`：
     - `/api/webhooks/status` 返回：
       - `webhook_path=/github-package-update/svc-alpha`
       - `webhook_url=https://example.com/github-package-update/svc-alpha`
     - UI 显示与 “复制 URL” 按钮都使用完整 URL，而非裸路径。
   - 未设置 `PODUP_PUBLIC_BASE_URL` 时：
     - UI 使用 `window.location.origin` 作为前缀构造完整 URL；
     - 在本地测试中应为 `http://127.0.0.1:25211/github-package-update/svc-alpha`。

4. **镜像锁列表与释放**
   - 在 state DB 中预插入若干 `image_locks` 记录：
     - Webhooks 页面列出所有锁，按 `acquired_at` 排序；
     - 每行显示 bucket / acquired 时间 / age 秒数 / 预计解锁时间；
     - 点击 “释放”：
       - 调用 `DELETE /api/image-locks/<bucket>`；
       - 行从表格中移除，且出现 “锁已释放” Toast。

5. **查看事件跳转**
   - 点击某个单元卡片的 “查看事件”：
     - URL 中增加 `?path_prefix=/github-package-update/<slug>`；
     - 前端路由跳转 `/events`，Events 页面展示过滤后的事件列表（在有事件的预热环境中验证）。

#### D. Events 事件与审计（events.spec.ts）

1. **列表加载与分页**
   - 通过 CLI 或专用 helper 预填 `event_log` 若干条记录：
     - Events 页面显示 `共 N 条 · 第 1 页`；
     - 当超过默认 page size 时，下一页按钮可用，内容分页展示。

2. **过滤器行为**
   - 分别在 Request ID / Path prefix / Status / Action 输入条件：
     - URL querystring 与输入字段同步；
     - 列表只显示符合条件的记录；
     - 清空条件时恢复完整列表。

3. **详情抽屉**
   - 点击任意一行：
     - 右侧 “详情” 区域显示：
       - action、method/status、path、时间、request_id、duration；
       - meta JSON 以 pretty 格式展示。
   - 未选择行时显示提示“选择左侧任意一行以查看详细元数据”。

4. **CSV 导出**
   - 当当前页有事件时：
     - 点击 “导出当前页 CSV”，触发浏览器下载；
     - 使用 Playwright 下载 API 或 network 拦截检查 CSV 内容包含表头、行数匹配当前页事件数。

#### E. Maintenance 维护工具（maintenance.spec.ts）

1. **状态目录检查**
   - 根据临时 state 目录真实文件：
     - `pod-upgrade-trigger.db`、`last_payload.bin`、`web/dist` 的存在/缺失状态与 UI badge 一致；
     - 备注中显示合理的 size / timestamp 信息。

2. **速率限制清理**
   - 预填 `rate_limit_tokens` 与 `image_locks`；
   - 输入 “最大保留时间（小时）” 值，点击 “清理”：
     - 触发 `POST /api/prune-state`；
     - 返回成功后出现 Toast 文案，包含 tokens/locks removed 数量；
      - 在 Tasks 页面可以看到对应的 `kind = "maintenance"` 任务及其执行日志；
      - 再访问 Events 页面，可以看到对应的 `prune-state-api` 事件（在有事件的预热环境中验证）。

3. **下载调试包**
   - 通过后端制造一次 GitHub HMAC 签名失败，使 `last_payload.bin` 存在；
   - 点击 “下载 last_payload.bin”：
     - 发起 `GET /last_payload.bin`；
     - 下载成功，文件非空。

#### F. Settings 配置总览（settings.spec.ts）

1. **环境变量展示**
   - 根据当前 env 设置：
     - `PODUP_STATE_DIR / PODUP_TOKEN / PODUP_MANUAL_TOKEN / PODUP_GH_WEBHOOK_SECRET` 的 configured/missing 与 UI 状态一致；
     - secret 变量值使用 `***` 掩码显示。

2. **systemd 单元列表**
   - 显示 auto-update unit 以及 `PODUP_MANUAL_UNITS` 中所有单元；
   - 每行的 “手动触发” 链接跳转 `/manual`，页面滚动到对应行（可选）。

3. **ForwardAuth 信息**
   - Dev 环境（`DEV_OPEN_ADMIN=1`）：
     - Header 显示 `(not configured)`；
     - Admin value configured 为 `no`；
     - DEV_OPEN_ADMIN 为 `true`；
     - Mode 为 `open`。
   - 严格模式环境：
     - Header 为配置值；
     - Admin value configured 为 `yes`；
     - DEV_OPEN_ADMIN 为 `false`；
     - Mode 为 `protected`。

#### G. 401 未授权页面（auth.spec.ts，可选）

1. **401 渲染与交互**
   - 在 ForwardAuth 严格模式下访问 `/settings` 等受保护页面：
     - UI 显示 `/401` 未授权提示；
     - 提示中显示原始请求路径；
     - “刷新重试” 按钮调用 `window.location.reload()`。

### 5. CI 集成建议

- 新增 `scripts/test-ui-e2e.sh`：
  1. 构建后端 release/debug 二进制；
  2. 构建前端 `web/dist`；
  3. 启动带 mock 环境的 `http-server`；
  4. 在 `web/` 下运行 `npx playwright test`；
  5. 结束后停止 `http-server`。
- 在 CI workflow 中添加 `ui-e2e` job：
  - 依赖 Rust/Node、Playwright 浏览器依赖；
  - 执行 `./scripts/test-ui-e2e.sh`；
  - 失败时上传 `ui-e2e-http.log`、Playwright 报告等 artifacts。

完成上述用例与脚本后，即可认为“前端 UI 自动化 E2E 测试”初版完成，后续新增功能应在对应模块下补充或扩展用例。EOF

#### E. Tasks 任务中心命令日志（tasks.spec.ts）

1. **任务列表与详情抽屉**
   - 使用 Playwright 在 mock 模式下访问 `/tasks?mock=enabled`：
     - 断言任务列表加载成功，`nightly manual upgrade` 等种子任务出现；
     - 点击任务行后，右侧抽屉展示类型、状态、起止时间、摘要、触发来源与 unit 状态。

2. **命令级日志与命令输出折叠**
   - 在 mock 模式下，某些日志携带结构化命令 meta（见 docs/task-management-panel.md 4.2.2）：
     - `action = "image-pull"`，`meta.command = "podman pull ..."`，包含 stdout/stderr/exit 等字段；
     - `action = "restart-unit"`，`meta.command = "systemctl --user restart ..."`。
   - UI E2E 用例会在时间线中定位这些日志：
     - 展开“命令输出”折叠，断言页面包含完整 command 文本；
     - 在 mock 场景下，stdout/stderr 文本（例如 `pulling from registry.example...`、warning 行）可见。
   - 当真实后端暂未实现命令 meta 或 `/sse/task-logs` 时，该用例可以按环境跳过，以避免 CI 误报；但在 mock happy-path 场景下应保持强校验。

3. **停止 / 强制停止 / 重试**
   - 通过 stop/force-stop/retry 按钮驱动 `/api/tasks/:id/stop`、`/force-stop`、`/retry`：
     - 断言任务状态从 running 变为 cancelled/failed，或创建新的 retry 任务；
     - 断言时间线中追加对应日志（如 `task-cancelled`、`task-force-killed`）。

