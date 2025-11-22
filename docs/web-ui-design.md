# Web 界面设计

为 `pod-upgrade-trigger` 的内置前端拟定页面路由、模块组成与跳转关系。静态资源固定在内置 dist（优先 `PODUP_STATE_DIR/web/dist`，缺失时回退到打包目录），不再允许通过环境变量指向其他前端。

## 导航结构

- 顶部状态条：常驻 Token 输入/刷新、健康状态(`/health`)、调度器 interval/迭代次数、系统时间；全局可见。
- 左侧导航：
  - `/` Dashboard
  - `/manual` 手动触发控制台
  - `/webhooks` GitHub Webhook 面板
  - `/events` 事件与审计
  - `/maintenance` 维护工具
  - `/settings` 配置总览
  - `/401` 未授权提示页（仅生产环境可见）

## 路由与页面内容

### `/` Dashboard

- 健康卡片：`/health`、`/sse/hello` 探针状态；最近一次 `podman-auto-update` 成功时间；调度器开启状态。
- 事件时间线：最近 N 条 `event_log`（新增 `GET /api/events?limit=` API）；点击项跳到 `/events?request_id=...`。
- 速率限制提示：展示 `rate_limit_tokens` 近窗口的使用情况；若接近阈值，跳转按钮链接到 `/maintenance#ratelimit`。
- 资源存在性：检查 `data/pod-upgrade-trigger.db`、`last_payload.bin`、`web/dist` 是否存在，异常时提示跳到 `/maintenance`。

### `/manual` 手动触发控制台

- “触发全部”表单：映射 `POST /api/manual/trigger`，字段 `all/dry_run/caller/reason`；提交后在下方历史卡片展示请求与响应。
- “按单元触发”列表：通过新 `GET /api/manual/services` 拉取 slug；每行含输入 `image/caller/reason` 与触发按钮（`/api/manual/services/<slug>`）。
- “传统 Token 触发”模块：包装 `/auto-update`，用于兼容旧流程。
- 历史记录：保留最近触发记录，点击项跳转到 `/events?request_id=...`。

### `/webhooks` GitHub Webhook 面板

- 单元卡片：展示 `/github-package-update/<unit>` 与 `/github-package-update/<unit>/redeploy` URL、最近成功/失败时间、HMAC 校验状态（需 `GET /api/webhooks/status`）。
- 镜像速率与锁：读取 `image_locks`，列出被锁镜像、预计解锁倒计时，提供“释放”操作（对应新 `DELETE /api/image-locks/<name>`）。
- GitHub 配置提示：显示 `PODUP_GH_WEBHOOK_SECRET` 是否配置（true/false），链接到文档。

### `/events` 事件与审计

- 列表分页：查询 `event_log`（`GET /api/events?page=`），字段 `request_id/method/path/status/action/meta/created_at`。
- 过滤器：按路径前缀（如 `/api/manual`）、状态码、动作、时间范围过滤。
- 详情抽屉：点击行展开原始 JSON payload、响应摘要、关联 systemd 单元；可复制 `request_id`。
- 导出：当页导出 CSV；链接按钮跳到 `/maintenance#export`。

### `/maintenance` 维护工具

- 状态目录检查：列出 `data/pod-upgrade-trigger.db`、`last_payload.bin`、`web/dist` 更新时间与大小；缺失时标红。
- 速率限制清理：按钮调用 `POST /api/prune-state`（或 CLI 代理）并接受 `max_age_hours`；完成后刷新页面并弹出结果。
- 下载调试包：提供 `last_payload.bin` 下载链接；并提示仅对最近一次签名失败有效。
- Web 静态部署提醒：检测 `web/dist` 是否生产构建；缺失时提示运行 `npm run build`（或 bun/pnpm 对应命令）。

### `/settings` 配置总览

- 环境变量展示（只读值/布尔）：`PODUP_STATE_DIR`、`PODUP_TOKEN/PODUP_MANUAL_TOKEN` 已配置与否、`PODUP_GH_WEBHOOK_SECRET` 配置状态、调度器 interval/max-iterations。
- systemd 单元表：列出 `podman-auto-update.service` 与各业务 unit 名称，标记是否在 `trigger_unints`（sic）可见列表；跳转到 `/manual` 的对应单元操作。
- API 基础信息：显示后端版本、构建时间、当前数据库连接串；链接到 `/events`。
- ForwardAuth 信息：显示 `FORWARD_AUTH_HEADER`、`FORWARD_AUTH_ADMIN_VALUE`、`FORWARD_AUTH_NICKNAME_HEADER`、`ADMIN_MODE_NAME` 是否配置，便于排查登录问题；仅在生产模式展示。

### `/401` 未授权页

- 文案：提示“未登录或无权限”，提供刷新/重试按钮。
- 行为：在生产环境中，当后端返回 401 时，前端路由跳到 `/401`，展示提示后立即用 `history.replaceState` 把浏览器地址恢复为原始受控路由，避免污染 URL 历史。
- 显示当前请求的原始路径（仅前端内存保存），便于用户确认自己本来在访问哪里。

## 跳转关系与交互流

- 顶部状态条的健康/调度器异常提示点击跳到 `/maintenance`。
- Dashboard 时间线项点击跳到 `/events` 并带上 `request_id` 查询参数。
- “速率限制”提示条跳到 `/maintenance#ratelimit`；清理完成后将返回 `/` 并刷新额度数据。
- 手动触发提交后在页面内吐司提示，并追加到历史列表；点击历史项跳到 `/events`。
- Webhook 面板中的 URL 复制按钮，旁边“查看事件”跳到 `/events?path=/github-package-update/<unit>`。
- 维护页的下载/清理操作完成后，通过全局 toast 提示并保留在当前页；必要时自动刷新状态卡片。
- 登录态跳转：
  - 开发/测试环境：不校验登录，直接进入目标路由。
  - 生产环境：每次请求若缺少 ForwardAuth 头或值不满足管理员规则，则展示 `/401`，但 URL 立刻 `replaceState` 回原目标，用户刷新后仍留在原地址；当请求头满足 `FORWARD_AUTH_HEADER` 且其值等于 `FORWARD_AUTH_ADMIN_VALUE` 时视为管理员并恢复正常导航。

## API 依赖与需要补齐的后端接口

- `GET /api/events`：支持分页、过滤、按 request_id 查询；为 Dashboard/Events 使用。
- `GET /api/manual/services`：列出可触发的 unit 与可选默认镜像。
- `GET /api/webhooks/status`：返回各 unit 的最近触发时间、成功/失败状态、HMAC 校验结果。
- `GET /api/image-locks` 与 `DELETE /api/image-locks/<name>`：查询/释放镜像锁。
- `POST /api/prune-state`：触发清理任务（或提供 CLI 代理 HTTP 入口）。
- 认证标识：沿用 tavily-hikari 的 ForwardAuth 方案——配置 `FORWARD_AUTH_HEADER` 指定载有用户 ID 的请求头，`FORWARD_AUTH_ADMIN_VALUE` 定义管理员匹配值，`FORWARD_AUTH_NICKNAME_HEADER`（可选）为 UI 昵称，`ADMIN_MODE_NAME` 提供兜底昵称，`DEV_OPEN_ADMIN` 仅用于本地开发放开权限。后端需对 Admin-only API 校验该组合并返回 401；前端收到 401 后跳转 `/401` 并恢复历史。

以上路由若未实现，前端需用 mock 数据占位，后端实现后直接对接。
