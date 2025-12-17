# Services Deploy Console（/manual）规格说明

## 背景

现有路由 `/manual` 保持不变，但在 Web UI 中对外展示名称调整为 **Services**，并从“手动触发/重启”升级为**服务部署控制台**：

- “部署（deploy）”语义：对目标服务执行 `podman pull` + `systemctl restart`。
- 自动更新（auto-update）语义单独呈现，不与 Services deploy 混用。
- 历史查看统一通过 Events 页面（`/events`）完成。

## 目标

- 让管理员能在 UI 上**批量部署**（deploy all）或**按服务部署**（per-service deploy）。
- 批量部署的主路径为 `POST /api/manual/deploy`，并且 **不包含 auto-update 单元**。
- UI 一律走 Task 流程（非 dry-run 时创建 Task 并打开 task drawer），便于追踪、重试与统一审计。

## 范围与非目标

### 范围

- UI：`/manual` 页面在导航中标为 **Services**，提供：
  - Deploy all（批量部署，支持 dry-run）。
  - Per-service deploy（按服务部署，支持 dry-run）。
  - Auto-update 独立卡片（`POST /api/manual/auto-update/run`）。
  - 将历史与详情导向 `/events` 与 `/tasks`（抽屉）。
- 后端：提供并维护以下 API 契约（详见下文）。

### 非目标

- 不做“仅部署有更新的服务”（no “deploy only updated services” yet）。
- UI 不提供“仅重启（restart-only）”入口（legacy API 仅为兼容保留）。

## 术语与定义

- **Service（服务）**：`GET /api/manual/services` 返回的条目，通常对应一个 systemd unit。
- **Deployable service（可部署服务）**：满足以下条件的 service：
  - `is_auto_update == false`（非 auto-update 单元）；且
  - `default_image` 存在（即该服务有可用于 `podman pull` 的默认镜像）。

## API 契约

### 1) `GET /api/manual/services`

用途：作为 Services 页面与部署逻辑的**源数据**，提供 service 列表以及 `default_image` 与 `is_auto_update` 等关键信息。

约束：
- 需要管理员权限（ForwardAuth）。

关键字段（示例，仅列出与本 spec 强相关部分）：

```json
{
  "services": [
    {
      "slug": "svc-alpha",
      "unit": "svc-alpha.service",
      "display_name": "svc-alpha.service",
      "default_image": "ghcr.io/acme/svc-alpha:stable",
      "is_auto_update": false
    }
  ]
}
```

### 2) `POST /api/manual/deploy`（批量 deploy）

用途：Services 页面 “Deploy all” 的主路径。

请求体：

```json
{
  "all": true,
  "dry_run": false,
  "caller": "ops",
  "reason": "rollout"
}
```

语义：
- 目标集合为：`manual_unit_list()` 里的单元 **去除 auto-update unit**，并且 **仅包含存在 `default_image` 的单元**。
- 缺少 `default_image` 的单元会被 `skipped`（不会回退成 restart-only）。
- `dry_run=true`：
  - **不创建 Task**；
  - 返回 202，列出 `deploying`（标记 dry-run）与 `skipped`，并包含 `request_id`（不返回 `task_id`）。
- `dry_run=false`：
  - 创建 Task 并异步执行；
  - 返回 202，包含 `task_id`，并将 `deploying` 标记为 `pending`。

响应体（dry-run，示例）：

```json
{
  "deploying": [
    {
      "unit": "svc-alpha.service",
      "image": "ghcr.io/acme/svc-alpha:stable",
      "status": "dry-run"
    }
  ],
  "skipped": [
    { "unit": "podman-auto-update.service", "status": "skipped", "message": "auto-update-unit" }
  ],
  "dry_run": true,
  "request_id": "req_xxxxx"
}
```

响应体（非 dry-run，示例）：

```json
{
  "deploying": [
    { "unit": "svc-alpha.service", "image": "ghcr.io/acme/svc-alpha:stable", "status": "pending" }
  ],
  "skipped": [
    { "unit": "podman-auto-update.service", "status": "skipped", "message": "auto-update-unit" }
  ],
  "dry_run": false,
  "task_id": "tsk_xxxxx",
  "request_id": "req_xxxxx"
}
```

### 3) `POST /api/manual/services/:slug`（按服务 deploy）

用途：Services 页面中单个服务行的 Deploy 按钮。

请求体：

```json
{
  "dry_run": false,
  "image": "ghcr.io/acme/svc-alpha:stable",
  "caller": "ops",
  "reason": "hotfix"
}
```

语义：
- `dry_run=true`：保持原有 dry-run 行为（不创建 Task）。
- `dry_run=false`：创建 Task 并异步执行，返回 202 并包含 `task_id`。UI 会始终传入 `image`（默认使用 `default_image`，仅在需要时手动覆盖），以确保执行 `podman pull` + `restart` 的部署语义。
  - 若 `image` 为空：后端将跳过 `podman pull`（等价于 restart-only），但 **UI 不允许** 以这种方式触发部署。

### 4) `POST /api/manual/auto-update/run`（auto-update 卡片）

用途：Services 页面中 “Auto-update” 独立卡片的运行入口（与 batch deploy 分离）。

请求体：

```json
{ "dry_run": false, "caller": "ops", "reason": "maintenance" }
```

语义要点：
- 可返回 `already-running`（202）用于提示同一 auto-update 单元正在运行。
- 非 dry-run 会创建 Task 并返回 `task_id`。

### 5) `POST /api/manual/trigger`（legacy：restart-only）

说明：兼容旧客户端与脚本的历史接口，语义为 **restart-only**，不作为当前 UI 的主路径。

建议在文档与示例中明确标注为 legacy，并引导新流程使用：
- 批量 deploy：`POST /api/manual/deploy`
- 按服务 deploy：`POST /api/manual/services/:slug`

## UI 行为摘要（仅概览）

- 左侧导航与页面标题：`/manual` 显示为 **Services**（路由不变）。
- 页面结构：
  - Deploy all 卡片：调用 `POST /api/manual/deploy`（支持 dry-run）。
  - Services 列表：数据来自 `GET /api/manual/services`；每行 Deploy 调 `POST /api/manual/services/:slug`。
  - Auto-update 卡片：调用 `POST /api/manual/auto-update/run`（与 deploy 分离）。
- 非 dry-run 的请求返回 `task_id` 时：
  - UI 打开 task drawer（任务抽屉）显示任务状态与日志；
  - 历史/审计通过跳转到 `/events`（按 `request_id` 或 path 过滤）查看。
