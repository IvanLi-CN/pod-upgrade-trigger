# 项目设计概览

`pod-upgrade-trigger`（可执行文件名 `pod-upgrade-trigger`）是一套围绕 Podman/systemd 自动升级流程构建的多通道触发系统。该二进制在同一进程中承担网络监听、鉴权、速率限制、静态资源托管、CLI 管理工具以及后台调度等职责。本设计文档拆解主要模块及其交互方式，方便后续扩展与运维追踪。

## 总体结构

1. **HTTP Frontend（常驻监听服务器）**
   - 通过 `http-server` 子命令在 `PODUP_HTTP_ADDR`（默认 `0.0.0.0:25111`）上监听 TCP 连接。
   - 对每个进入的连接派生子进程运行 `server` 子命令，在该子进程内从 STDIN 读取请求行、头、主体并写回响应。
   - 支持 `/health`、`/sse/hello`、GitHub webhook 路由、传统 `/auto-update` 触发入口（兼容旧流程）以及 `/api/manual/*` JSON API。
   - 根据路径决定后续处理逻辑，并通过统一的 `RequestContext` 承载 method/path/query/body 等信息。

2. **GitHub Webhook 处理**
   - `handle_github_request` 校验 `x-hub-signature-256`，解析包体判别容器镜像（对 `package` 与 `registry_package` 两种 schema 兼容）。
   - 对镜像名执行速率限制，并通过 `systemd-run --user` 异步拉起 `--run-task` 子命令，以便后台执行 `podman pull + systemctl restart`。
   - 支持 `/github-package-update/<unit>` 与 `/github-package-update/<unit>/redeploy` 两种 URL。

3. **手动触发 API 与 CLI**
   - `/auto-update`：历史兼容的 token 触发路径，主要启动 `podman-auto-update.service`。
   - `/api/manual/trigger`：POST JSON，支持 `all/units/dry_run/caller/reason` 等字段，可批量触发或纯 dry-run。
   - `/api/manual/services/<slug>`：面向单个 unit 的 JSON API，可附加 `image` 以提前拉取镜像。
   - CLI 子命令：`server`（守护进程）、`scheduler`、`trigger-units`、`trigger-all`、`prune-state`、`run-task` 与 HTTP API 共享实现，便于脚本化集成。

4. **后台调度器**
   - `--scheduler` 在独立 CLI 进程内运行，按固定时间片（默认 15 分钟，可通过 CLI / 环境变量覆盖）轮询触发 `podman-auto-update.service`。
   - 支持 `--max-iterations`，方便在 CI 或短期任务中执行有限次数。

5. **速率限制与状态维护**
   - `/auto-update` 入口使用 SQLite 中的 `rate_limit_tokens` 表记录触发时间戳（双窗口限制）；
   - GitHub 镜像级别限制与并发锁也落在 SQLite（`rate_limit_tokens` + `image_locks`），保证同一镜像在指定窗口内的触发次数；
   - 提供 `--prune-state` 命令清理旧令牌、过期锁，以及历史遗留的目录文件。

6. **静态资源托管**
   - `try_serve_frontend` 会在 `PODUP_STATE_DIR/web/dist`、当前工作目录下的 `web/dist`、编译产物内置的 `/srv/app/web` 等固定位置查找 UI 资源并托管到 `/`、`/assets/*`、`/favicon.ico` 路径，不再允许通过环境变量指向其他前端构建。

7. **安全与鉴权**
   - GitHub Webhook（`/github-package-update/*`）依赖 `PODUP_GH_WEBHOOK_SECRET` 进行 HMAC 校验；该接收端点不走 ForwardAuth/CSRF。
   - 管理/手动触发类 API（主要是 `/api/*`，以及 legacy `/auto-update`）使用 ForwardAuth（`PODUP_FWD_AUTH_HEADER` + `PODUP_FWD_AUTH_ADMIN_VALUE`）鉴权；本地开发可用 `PODUP_DEV_OPEN_ADMIN=1` 绕过。
   - 对会产生副作用的 Admin API（`POST/PUT/PATCH/DELETE`）额外强制要求 `x-podup-csrf: 1`；若包含 JSON body，还要求 `Content-Type: application/json...`（前缀匹配）。
   - 响应内容通过 `respond_*` 系列函数集中封装，便于统一返回体与事件记录。

8. **事件追踪**
   - `log_audit_event`、`log_simple_audit` 直接调用 `persist_event_record`，所有 HTTP 请求都写入 SQLite `event_log` 表。
   - CLI / 调度器使用 `record_system_event` 记录非 HTTP 的触发历史。

## 模块依赖关系

```
RequestContext
    ├── respond_* (统一响应 + 事件记录)
    ├── handle_manual_api / handle_manual_request / try_serve_frontend
    └── handle_github_request

Background operations
    ├── run_scheduler_loop (调度器)
    ├── run_trigger_cli / trigger_units / trigger_single_unit
    └── run_background_task (systemd-run 子任务)

Persistence & Rate limits
    ├── rate_limit_check / enforce_rate_limit
    ├── check_github_image_limit / enforce_github_image_limit
    ├── prune_state_dir (维护命令)
    └── persist_event_record / record_system_event (数据库事件记录)
```

## 数据持久化

- **State Dir**：依旧保留 `ratelimit.db`、`github-image-*` 等纯文本数据库，负责限流与锁机制。
- **SQL 数据库**：默认连接 `sqlite://data/pod-upgrade-trigger.db`（可用 `PODUP_DB_URL` 覆盖），启动时使用 `sqlx::migrate!` 自动执行 `migrations/` 目录中的脚本；同一个数据库同时保存请求事件（`event_log`）、限流令牌（`rate_limit_tokens`）以及镜像锁（`image_locks`）。

## 扩展点

- 若未来接入更多 Webhook 平台，可直接复用 `lookup_unit_from_path` 与 `spawn_background_task`。
- 需要新的限流策略时，可在 `rate_limit_check` / `check_github_image_limit` 基础上扩展更多 db 文件。
- 通过在 `systemd` 目录追加 `.timer`、`.service` 可快速部署其它自动刷新任务。
- SQL 迁移方案确保对 schema 的新增字段/索引可以演进更新。

## 近期运维改进（2025-11）

1. 自动发现结果持久化与暴露
   - 自动发现的 Podman auto-update 单元需要在 DB 初始化后落库并被 `/api/manual/services` 返回，且与 `PODUP_MANUAL_UNITS` 的手工列表区分存储/展示。
2. 状态 API 与自检
   - `/api/webhooks/status`、`/api/image-locks` 要在启动自检或查询失败时返回结构化错误（如 DB 不存在/不可写、Podman 连接异常），同时写入日志，避免 502 静默。
3. 状态库自愈与健康页提示
   - SQLite 状态库缺失或无写权限时应自动创建/迁移；无法自愈时在 `/health` 给出路径和环境变量指引，便于运维修复。
4. Settings 可视化
   - Settings 页新增“发现的 auto-update 单元数量 + 摘要列表”，并与环境变量配置的手工单元并排展示供核对。
