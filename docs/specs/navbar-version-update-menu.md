# 顶栏版本展示与手动自更新入口（下拉菜单）

## 背景

Web UI 顶栏目前已具备“新版本可用”的提示（基于 `GET /api/version/check`），但：

- 顶栏未展示“当前运行版本号”，不利于主人快速确认部署版本；
- “新版本可用”目前仅提供跳转 Release 页，缺少“立即更新”的入口；
- 更新期间服务端可能重启，UI 需要明确“如何查看更新结果/状态”，避免误以为更新失败。

本工作项在不改变现有更新脚本链路的前提下，补齐顶栏版本信息与“手动触发自更新”的入口。

## 目标

- 顶栏大标题右侧**始终显示当前版本号**（`vX.Y.Z`）。
- 当存在新版本时，顶栏额外显示**新版本号入口**；点击后展示下拉菜单：
  1. “立即更新”（带二次确认）
  2. “跳转到该版本代码页”（GitHub `tree/<tag>`）
- 点击“立即更新”确认后，触发一次自更新，并**自动跳转到 `/tasks`** 以便主人观察更新结果。

## 范围

### 前端（Web UI）

- 顶栏增加“当前版本号”展示（`vX.Y.Z`）。
- 将“新版本可用”提示改为下拉菜单触发点：
  - 菜单项：立即更新（需要二次确认）
  - 菜单项：打开 GitHub 代码页（`/tree/<tag>`）
- 触发“立即更新”后：
  - 显示 toast（提示更新可能导致服务短暂重启）
  - 跳转到 `/tasks`

### 后端（HTTP API）

- 新增：`POST /api/self-update/run`
  - 触发一次“自更新执行器”（即 `PODUP_SELF_UPDATE_COMMAND` 指定的命令）
  - **更新到执行时最新 release**（不指定 tag；由脚本自行取 `releases/latest`）
  - **不强制真实更新**：若配置了 `PODUP_SELF_UPDATE_DRY_RUN=1`，则保持 dry-run 行为

## 非目标

- 不引入“更新进度条/实时日志面板”；结果以 `/tasks`（以及可能的系统日志）为准。
- 不改造现有脚本行为（`scripts/self-update-runner.sh`、`scripts/update-pod-upgrade-trigger-from-release.sh`）。
- 不新增复杂的版本通道策略（beta/stable 等）。

## 关键用户流程

### 1) 查看当前版本

1. 主人打开任意页面；
2. 顶栏标题右侧显示当前版本号（`vX.Y.Z`）。

约束：该信息**不依赖 GitHub 可用性**，避免 GitHub 失败导致当前版本不显示。

### 2) 有新版本时的交互

1. `useVersionCheck()` 检测到 `hasUpdate === true` 且存在 `latestTag`；
2. 顶栏显示新版本号入口；
3. 主人点击入口打开下拉菜单：
   - “立即更新”
   - “跳转到该版本代码页”

### 3) 立即更新（带二次确认）

1. 主人点击“立即更新”；
2. UI 弹二次确认：
   - 提示：会触发自更新，服务可能重启，页面短暂不可用；
   - 提示：若 `PODUP_SELF_UPDATE_DRY_RUN=1` 则仅验证下载/校验，不替换二进制、不重启（行为由脚本决定）。
3. 主人确认后，UI 调用 `POST /api/self-update/run`；
4. 请求返回后 UI 跳转到 `/tasks`。

说明：`/tasks` 列表页面本身会定时轮询 `/api/tasks`，因此服务重启导致短暂失败时会自动恢复；无需额外“自动重试”即可看到后续导入的自更新结果记录。

## 数据 / 领域模型变更

本工作项不新增数据库表结构。

自更新任务记录沿用既有 `tasks` / `task_units` / `task_logs`：

- `POST /api/self-update/run` 会先创建一条可追踪的任务记录（kind=`maintenance`、meta.type=`self-update-run`），并返回 `task_id`；
- 后端通过现有 task executor 调度 `run-task` 去执行 `PODUP_SELF_UPDATE_COMMAND`，由 worker 将任务最终状态更新为 `succeeded/failed` 并收敛日志。

另外：仓库既有的“自更新报告导入”机制仍可工作——当自更新执行器（例如 `scripts/self-update-runner.sh`）生成 JSON 报告时，HTTP server 的 importer 线程会把报告导入到 `tasks` 表，kind=`self-update`。这类“报告导入任务”与手动 API 触发的 `maintenance` 任务是两条链路，可能同时出现。

## 接口设计

### 1) 获取当前版本

- `GET /api/settings`
  - 使用字段：`version.release_tag`（推荐）作为 `vX.Y.Z` 的展示源。
  - 该接口不依赖 GitHub。

### 2) 检测新版本

- `GET /api/version/check`
  - 使用字段：`latest.release_tag` + `has_update`

### 3) 触发一次自更新（新增）

#### `POST /api/self-update/run`

- 鉴权/保护：
  - 与其它管理接口一致：需 admin（ForwardAuth 或 open-admin 模式）
  - 需要 CSRF header：`X-Podup-CSRF: 1`
- Request body：
  - 可为空（`Content-Length: 0`）
  - 或者发送 `{}`（需 `Content-Type: application/json`）
- 行为：
  - 若 `PODUP_SELF_UPDATE_COMMAND` 未配置或不可执行：
    - 返回 503，并给出可读错误信息；
  - 否则：
    - 触发执行器运行一次（不指定 tag → 更新到执行时 latest release）
    - 返回 202，并返回 `task_id`（用于 UI 跳转 `/tasks` 跟踪）
  - `dry_run`：由后端读取 `PODUP_SELF_UPDATE_DRY_RUN` 决定，并在响应中回显（便于 UI 提示）。

响应（示例）：

```json
{
  "status": "pending",
  "message": "scheduled via task",
  "task_id": "tsk_...",
  "dry_run": true,
  "request_id": "req_..."
}
```

错误响应（示例）：

```json
{
  "error": "self-update-command-missing",
  "message": "Self-update command is not configured",
  "required": ["PODUP_SELF_UPDATE_COMMAND"]
}
```

## 模块边界（实现提示）

- UI：
  - 顶栏组件负责展示版本与触发交互；
  - 版本数据来源分离：
    - 当前版本：`/api/settings`
    - 新版本：`useVersionCheck()`（`/api/version/check` + 1h 节流）
- 后端：
  - 新增路由处理函数，复用已有 `run_self_update_command()` 或其封装；
  - 保持与 scheduler 的行为一致（尊重 dry-run、超时与日志输出）。

## 兼容性与迁移

- 若未配置 `PODUP_SELF_UPDATE_COMMAND`：
  - 顶栏仍可显示当前版本与“新版本可用”提示；
  - 点击“立即更新”会失败并提示“自更新未配置”。
- GitHub 不可用：
  - 当前版本仍可显示；
  - 新版本提示可能不可用（取决于 `/api/version/check`）。

## 风险点

- **运行时重启**：执行器可能会重启 `pod-upgrade-trigger-http.service`，导致 UI 短暂 5xx/断连；通过跳转 `/tasks` + 列表轮询可缓解体验。
- **任务可见性**：`POST /api/self-update/run` 会立即创建任务并返回 `task_id`；若执行器本身还会生成“自更新报告”，则 importer 可能额外导入一条 kind=`self-update` 的任务记录（属既有机制）。
- **systemd 依赖**：默认脚本使用 `systemctl --user restart ...`，若部署形态不满足 user systemd 或 unit 名不同，会导致更新失败（属既有约束）。

## 测试计划（建议）

- 前端：
  - 顶栏能显示当前版本（`vX.Y.Z`），且 GitHub 不可用时仍显示；
  - 有新版本时，下拉菜单两项可见，且代码页链接正确；
  - “立即更新”需二次确认，确认后跳转 `/tasks`。
- 后端：
  - 未配置 `PODUP_SELF_UPDATE_COMMAND`：`POST /api/self-update/run` 返回明确错误；
  - 配置 `PODUP_SELF_UPDATE_DRY_RUN=1`：响应回显 `dry_run=true`；
  - 在 mock/dev 环境触发一次 dry-run，观察 importer 导入的 `self-update` 记录出现在 `/api/tasks`。
