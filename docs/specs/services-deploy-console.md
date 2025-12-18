# Services Deploy Console（/services）规格说明

## 背景

Services Deploy Console 是 Web UI 的“服务部署控制台”，用于对目标服务执行 deploy（`podman pull` + `systemctl restart`）并统一进入 Task 流程跟踪。

本次改动将页面路由从 `/manual` 迁移为 `/services`（语义更清晰）；同时增强 Services 页内的“任务中心抽屉”（Task Drawer）以支持可分享的 URL 定位、遮罩关闭等能力。

- “部署（deploy）”语义：对目标服务执行 `podman pull` + `systemctl restart`。
- 自动更新（auto-update）语义单独呈现，不与 Services deploy 混用。
- 历史查看统一通过 Events 页面（`/events`）完成。

## 目标

- 让管理员能在 UI 上**批量部署**（deploy all）或**按服务部署**（per-service deploy）。
- 批量部署的主路径为 `POST /api/manual/deploy`，并且 **不包含 auto-update 单元**。
- UI 一律走 Task 流程（非 dry-run 时创建 Task 并打开 task drawer），便于追踪、重试与统一审计。
- Task Drawer 支持：
  - 点击遮罩关闭；
  - URL 体现抽屉当前状态（列表 / 详情 + task_id），可复制分享定位；
  - 任务详情中展示 `task_id`；
  - 提供跳转到 Tasks 模块对应页面的入口（`/tasks?task_id=...`）。

## 范围与非目标

### 范围

- UI：`/manual` 页面在导航中标为 **Services**，提供：
  - Deploy all（批量部署，支持 dry-run）。
  - Per-service deploy（按服务部署，支持 dry-run）。
  - Auto-update 独立卡片（`POST /api/manual/auto-update/run`）。
  - 将历史与详情导向 `/events` 与 `/tasks`（抽屉）。
- 后端：提供并维护以下 API 契约（详见下文）。
- 路由迁移与兼容：
  - 新页面路由为 `/services`；
  - 旧 `/manual` 仍可访问，但将重定向到 `/services`（前端路由级兼容）。
- Task Drawer 的 URL 交互（同页内 deep link）与 task_id 格式调整（详见下文）。

### 非目标

- 不做“仅部署有更新的服务”（no “deploy only updated services” yet）。
- UI 不提供“仅重启（restart-only）”入口（legacy API 仅为兼容保留）。
- 不调整现有有副作用 API 的路径前缀：仍保持 `/api/manual/*`（本次只改 Web UI 页面路由）。
- 不对历史任务的 `task_id` 做迁移或重写（仅影响新创建的任务）。

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

## 路由与 URL 兼容

- 新页面路由：`/services`
- 兼容入口：`/manual`（重定向到 `/services`）

> 说明：本次仅调整 Web UI 页面路由；后端 API 前缀保持 `/api/manual/*` 不变。

## Task Drawer 交互与 URL（Services 页内）

### URL 约定

在 `/services` 页内，使用 query 参数表达抽屉状态，便于复制链接定位：

- 抽屉列表：`/services?drawer=tasks`
- 抽屉详情：`/services?drawer=tasks&task_id=<task_id>`

约定行为：

- 当 URL 进入“详情态”时，页面应自动打开 Task Drawer 并加载对应任务详情。
- 当 URL 进入“列表态”时，页面应自动打开 Task Drawer 并显示任务列表。
- 关闭 Task Drawer 时，应移除 `drawer` 与 `task_id`（回到无抽屉状态）。

### 关闭与遮罩

- 点击抽屉右上角 close 按钮关闭（现有行为保留）。
- 点击遮罩区域关闭（新增）。
- 点击抽屉内容区域不触发关闭。

### 导航与回退策略

- 推荐使用 `replace` 更新 query（避免频繁切换详情时污染浏览器历史）；如需更强“回退关闭/回到列表”体验，可在实现阶段再评估改为 push。

## 任务详情补齐（Services 抽屉）

- 任务详情区域增加 `task_id` 展示（建议等宽字体 + 一键复制）。
- 增加跳转入口：
  - 当处于详情态时，提供按钮跳转到 `/tasks?task_id=<task_id>`（打开 Tasks 模块的同一任务详情抽屉）。
  - 当处于列表态时，可选提供“打开 Tasks 页面”入口（不带 task_id）。

## task_id 生成规则（后端）

### 目标

- 新创建任务的 `task_id` 使用 nanoid，且字符集对 OCR 友好（避免易混字符）。
- 保留现有前缀语义：
  - 普通任务：`tsk_<nanoid>`
  - 重试任务：`retry_<nanoid>`

### 字符集与长度

- 建议 alphabet（OCR 友好，去除 0/O、1/I/L、2/Z、5/S、6/G、8/B 等易混字符）：
  - `3479ACDEFHJKMNPQRTUVWXY`
- 建议长度：16（约 72 bits 熵；在本项目规模下碰撞概率可忽略）。

> 注：`task_id` 会出现在 URL、日志与本地文件名（pid file）中，因此需要保持 URL-safe 与文件名安全字符集。

## UI 行为摘要（仅概览）

- 左侧导航与页面标题：`/services` 显示为 **Services**（`/manual` 重定向）。
- 页面结构：
  - Deploy all 卡片：调用 `POST /api/manual/deploy`（支持 dry-run）。
  - Services 列表：数据来自 `GET /api/manual/services`；每行 Deploy 调 `POST /api/manual/services/:slug`。
  - Auto-update 卡片：调用 `POST /api/manual/auto-update/run`（与 deploy 分离）。
- 非 dry-run 的请求返回 `task_id` 时：
  - UI 打开 task drawer（任务抽屉）显示任务状态与日志；
  - 历史/审计通过跳转到 `/events`（按 `request_id` 或 path 过滤）查看。

## 测试计划（实现阶段）

- Web UI（Playwright）：
  - 更新原 `/manual` 相关用例到 `/services`（或保留 `/manual` 访问并断言重定向）。
  - 新增/更新用例覆盖：
    - 打开抽屉后 URL 进入 `drawer=tasks`；
    - 点击任务进入详情后 URL 带 `task_id`；
    - 直接访问详情 URL 能恢复抽屉与详情；
    - 点击遮罩关闭抽屉并清理 URL。
- 后端（Rust）：
  - 为 task_id 生成函数增加单测：前缀、长度、字符集约束、随机性（至少 smoke）。
- E2E：
  - 更新 UI/E2E 测试脚本中对路径与 task_id 形态的断言（如存在）。
