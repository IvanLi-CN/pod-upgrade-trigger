# 任务管理面板设计

## 一、背景与目标

- 系统目前已经具备：
  - 事件日志（`/api/events` + Events 页面），可以按请求查看结果与元数据；
  - 手动触发、Webhook、Scheduler 等多种触发方式；
  - 后端内部的异步执行机制（通过 `systemd-run --collect` 启动 transient unit，或 `--run-task` 内联执行）。
- 现状问题：
  - 没有面向“异步任务”的统一视图，只能从事件日志或系统日志侧面观察；
  - 无法直观看到“当前有哪些任务在跑”、“任务最终是否成功”、“是否可以停止”；
  - 日志分散（系统 stdout / journal + `event_log`），排障成本较高。
- 任务管理面板目标：
  - 提供一个统一的“任务中心”视图：可列出任务、查看详情与日志、执行控制（停止等）；
  - 让运维 / 开发在 UI 上快速回答：
    - 当前有哪些异步任务、状态如何；
    - 某个任务的执行轨迹和输出信息是什么；
    - 某个任务能否停止、是否已经完成。
  - 与现有事件日志体系兼容，尽量复用已有记录能力。

---

## 二、任务范围与抽象

### 2.1 任务类型

统一抽象“任务（Task）”，涵盖（但不限于）：

- GitHub Webhook 触发的后台更新任务（通过 `systemd-run` 启动）；
- 手动触发的批量更新任务（一次触发多个 unit）；
- Scheduler 定时触发的自动更新任务；
- 维护类长耗时操作（例如大规模 prune / 检查 / 迁移，后续可扩展）；
- 其他内部自动任务（未来新增的后台校验 / 统计等），同样需要纳入任务面板统一展示。

### 2.2 任务粒度

- 粒度定义：一次触发产生的一串后台操作为一个 Task。
- 每个 Task 具有唯一的 `task_id`，并关联：
  - 任务类型（manual / github-webhook / scheduler / maintenance 等）；
  - 一个或多个底层 unit 操作（拉镜像、重启 unit 等）；
  - 一组事件日志记录（通过 `request_id` 或独立的任务键关联）。

---

## 三、角色与权限

- 访问控制复用现有 ForwardAuth 逻辑：
  - 列表、详情、日志查看：需要通过“管理员请求”判定（与 `/api/settings`、`/api/manual` 一致）；
  - 管理操作（停止任务等）：必须在管理员上下文中才允许。
- 未授权时：
  - 后端返回 `401 Unauthorized`；
  - 前端沿用现有处理逻辑（跳转 401 页面或通过 toast 给出提示）。

---

## 四、核心功能需求

### 4.1 任务列表

#### 4.1.1 基本列表

- 提供最近一定时间内的任务列表：
  - `task_id`
  - 任务类型（manual / github-webhook / scheduler / maintenance …）
  - 关联服务 / unit（单个或列表）
  - 触发来源（页面 / Webhook / Scheduler / CLI）
  - 触发时间 `created_at`
  - 当前状态：
    - `pending` / `running` / `succeeded` / `failed` / `cancelled` / `skipped`
  - 耗时：
    - 已完成任务：总耗时；
    - 运行中任务：当前已运行时长（前端根据 `now - started_at` 计算）。
  - 简要结果摘要（例如 `3/5 units succeeded, 2 failed`）。
- 分页：
  - 支持 `page`, `per_page`, `total`, `has_next`；
  - 默认 `per_page` 建议 20–50，风格对齐 `/api/events`。

#### 4.1.2 过滤与排序

- 过滤：
  - 按状态过滤：running / failed / succeeded 等；
  - 按任务类型过滤：只看 webhook / manual / scheduler / maintenance / 其他自动任务等；
  - 按关联 unit 过滤：输入服务名，支持前缀或模糊匹配；
  - 预留按时间范围过滤能力。
  - 快速分类切换：
    - 在列表上方提供一组“分类标签 / Segment 控件”，用于一键切换常用视图；
    - 例如：`全部` / `手动` / `Webhook` / `自动任务（Scheduler 等）` / `维护任务`；
    - 快速分类与“任务类型过滤”共用同一筛选条件，保证交互一致；
    - 可选：在分类标签上显示各类任务的计数，方便判断当前活跃的自动任务数量。
- 排序：
  - 默认按 `created_at` 倒序；
  - 可扩展按 `finished_at` 或耗时排序。

#### 4.1.3 实时性

- 前端定时轮询列表：
  - 例如每 5–10 秒请求一次最新列表，更新状态与耗时；
  - 控制并发请求与错误提示，避免对后端造成过多压力。

---

### 4.2 任务详情与日志

#### 4.2.1 任务详情视图

- 从列表点击某条任务，进入详情视图（页面模式或抽屉模式，下文详述）：
  - 显示：
    - `task_id`
    - 类型、状态、起止时间、耗时；
    - 触发来源（触发 API 路径、Scheduler iteration、触发页面等）；
    - 关联 unit 列表及各自状态（成功 / 失败 / 跳过），与 `UnitActionResult` 对齐；
    - 调用参数摘要（`caller`、`reason`、`image` 等），从任务记录或 `meta` 中提取。
- 对批量任务：
  - 对每个 unit 单独展示状态和错误信息；
  - 支持快速识别部分失败 / 全部成功。

#### 4.2.2 日志与轨迹

- “日志”以时间线形式展示任务的执行轨迹：
  - 基于内部可控数据源（`event_log` 表 + 任务表）；
  - 通过统一键（推荐：`task_id` 或 `task_ref`），查询同一任务相关的 event 记录。
- 展示层级：
  - 概要项：时间 + action + 状态 + 简短说明（例如 “image pull success”、“restart unit failed”）；
  - 详细项：展开后可以查看完整 `meta` JSON，用于问题排查。
- 命令级输出记录（新增约定）：
  - 需求：在任务详情中能够看到“原始执行的命令及其所有输出”，便于直接复制到终端重现问题。
  - 针对与任务强相关的外部命令（当前至少包括）：
    - 镜像拉取：`podman pull <image>`（手动服务 / GitHub Webhook / Scheduler 等）；
    - 单元操作：`systemctl --user start|restart <unit>`；
    - 任务停止：`systemctl --user stop|kill <unit>`。
  - 这些命令的执行结果统一通过 `task_logs.meta` 结构化记录，不新增表结构，推荐 JSON 形态：
    - `type: "command"`：用于前端快速识别“命令输出型”日志；
    - `command: string`：可复制的完整命令行（例如 `"podman pull ghcr.io/muety/wakapi:latest"`）；
    - `argv?: string[]`：拆分后的参数数组，便于后续程序化处理（可选）；
    - `stdout?: string`：命令的标准输出完整内容（必要时可裁剪并在 JSON 中标记 `truncated`）；
    - `stderr?: string`：命令的标准错误输出；
    - `exit?: string`：退出码的字符串表示，例如 `"exit=0"`、`"exit=42"`；
    - 其他字段（可选）：如 `attempt`、`unit`、`image` 等维持与现有 meta 一致。
  - Task 详情接口 `GET /api/tasks/:id` 在保持 `TaskLogEntry` 现有字段不变的前提下，只在 `meta` 中增加上述结构。旧数据（无 `command` 字段）仍然合法。
  - 前端渲染约定：
    - 时间线上仍以 `action` + `status` + `summary` 作为概览；
    - 当 `meta.type === "command"` 或 `meta` 中同时存在 `command` 与 `stdout` / `stderr` 时，在日志项内提供可折叠区域展示：
      - 一行只读的 `command` 文本（支持一键复制）；
      - 受高度限制的 `stdout` / `stderr` 文本框（滚动查看完整内容，必要时在 JSON 中用 `truncated` 标记被截断的输出）。
    - 未携带命令信息的历史日志仍按当前行为展示，不需要特殊处理。
  - 额外 UX 建议：对于处于 `running` 状态且被识别为命令型的日志项，前端可以在首次出现时默认展开一次“命令输出”折叠，以方便实时观察；用户手动收起后不再自动展开。
- 原始系统日志（systemd/journal）：
  - 当前版本不直接暴露；
  - 后续如需接入，需单独设计过滤与脱敏策略。

#### 4.2.3 状态更新

- 当任务为 `running` 状态时：
  - 详情视图应定时刷新（例如每 2–3 秒请求一次单任务详情接口）；
  - 当任务进入终态（succeeded / failed / cancelled / skipped）后停止轮询。

---
- 任务日志 SSE 通道（后端已实现并默认启用，用于实时日志流）
  - 当前服务已经在生产路径中实现基于 SSE 的任务日志流接口，强烈建议部署环境开启并保持可用；即便 SSE 临时不可用（如网络或代理不透传 SSE），前端仍可退回 HTTP 轮询保证功能可用。
  - 接口与事件形态：
    - `GET /sse/task-logs?task_id=<id>`；
    - `event: log`：`data` 为完整 `TaskLogEntry` JSON，同一 `id` 可以多次出现，表示该日志被更新（例如 stdout/stderr 被追加、status 从 `running` 变为 `succeeded`）；
    - `event: end`：当任务进入终态，或因超时、任务丢失、客户端断开等原因结束流式传输时发送一次，前端应关闭对应的 `EventSource`。
  - 按任务状态区分两种模式：
    - 非 `running` 任务（快照模式）：服务端在建立 SSE 连接后，一次性发送当前所有 `TaskLogEntry` 的 `event: log` 事件，并在末尾发送一次 `event: end`，可视为通过 SSE 返回日志快照。
    - `running` 任务（流式模式）：服务端写入一次 HTTP+SSE 头并保持连接，周期性调用 `load_task_detail_record(task_id)` 等内部查询，仅在发现新增或变更的 `TaskLogEntry`（按 `id` 判断 JSON 是否变更）时发送 `event: log`，直到任务进入终态或达到最大流式时长后发送 `event: end` 结束。
  - 推荐前端行为（当前实现已遵循）：
    - 初次打开任务详情时仍调用 `GET /api/tasks/:id` 获取完整快照（包含当前 `logs`）；
    - 对于 `status === "running"` 的任务，并行建立 `/sse/task-logs?task_id=...` 的 `EventSource`：
      - 每收到一条 `event: log`，按 `id` 将本地日志数组中的对应条目覆盖/追加；
      - 收到 `event: end` 或前端检测到任务进入终态后关闭 SSE；
    - 当 SSE 连接异常或不可用时，前端可以退回纯 HTTP 轮询模式，保障任务详情与日志仍然可用。


### 4.3 任务控制（停止等）

#### 4.3.1 停止 / 取消任务

- 对运行中的任务支持“停止 / 取消”操作：
  - 在列表和详情中对 `running` 任务展示“停止”相关操作；
  - 点击前弹出确认对话框，说明：
    - 优先尝试“优雅停止”（graceful stop），给任务预留清理与善后时间；
    - 如任务不支持或优雅停止未生效，再由用户显式触发“强制停止”。
- 停止级别：
  - 优雅停止：
    - 默认操作，为主要入口（按钮文案可直接使用“停止”）；
    - 后端尝试通过正常方式结束任务，例如对 systemd 后台任务执行 `systemctl --user stop <unit-name>`；
    - 成功后，任务进入 `cancelled` 终态并记录结束时间与原因摘要。
  - 强制停止：
    - 在优雅停止失败或任务标记为“不支持优雅停止”时，提供“强制停止”按钮；
    - 需要更明显的视觉区分和更强的二次确认文案（明确提示可能导致未完成的操作被中断）；
    - 后端使用更强硬的手段终止执行（例如向对应进程发送终止信号或等价能力），并记录一条明确的系统事件（action 如 `task-force-killed`）。
- 后端行为：
  - 任务记录中保存底层执行标识（如 systemd transient unit 名称）；
  - 所有停止尝试（无论优雅还是强制）都应写入事件日志，包含起因和结果；
  - 停止失败（任务已结束或系统错误）：
    - 不改变原终态，记录失败原因，并向前端返回可读错误信息。
- 幂等性：
  - 多次对同一任务发起停止请求时，应安全返回当前状态，不产生额外副作用。

#### 4.3.2 只读模式保护

- 在未通过管理员校验时：
  - 管理操作接口直接返回 401；
  - 前端仅展示任务列表 / 详情的只读视图（可作为后续迭代选项）。

#### 4.3.3 可选扩展操作

- 重试任务（本迭代内实现）：
  - 在失败或已完成的任务详情 / 抽屉中提供“重试”按钮；
  - 基于原始参数（任务类型、关联 unit、调用参数等）创建一个新 Task（新 `task_id`），并清晰标明“来自某任务的重试”关系；
  - 点击重试后，可直接自动打开新任务的抽屉详情，方便观察执行进度；
  - 原任务保持只读状态，避免状态被篡改。
- 导出任务日志（可选）：
  - 允许将该任务的日志 / 事件轨迹导出为 JSON / 文本格式；
  - 用于线下排查或在其他系统中分析。

---

## 五、后端设计约束（需求侧推导）

### 5.1 任务持久化模型

- 引入“任务表”或等价结构，字段建议包含：
  - `id` / `task_id`（主键或业务主键）；
  - `type`：任务类型（manual / github-webhook / scheduler / maintenance …）；
  - `status`：`pending` / `running` / `succeeded` / `failed` / `cancelled` / `skipped`；
  - `created_at`、`started_at`、`finished_at`、`updated_at`；
  - `units`：关联 unit 信息（可以为 JSON 列表或子表）；
  - `summary`：摘要信息（用于列表显示简短状态说明）；
  - 与 `event_log` 的关联键（`task_id` 或 `request_id`）。
- 任务生命周期：
  - 创建：触发任务时创建 Task 记录，初始状态 `pending` 或 `running`；
  - 更新：任务开始执行时标记 `running`，写入 `started_at`；
  - 完成：成功 / 失败 / 取消时进入终态，写入 `finished_at`，更新 `summary`。

### 5.2 日志关联能力

- 利用现有 `event_log`：
  - 增加任务维度键（例如字段 `task_id` 或在 `meta` 中嵌入 `task_id`），保证同一任务所有事件可查询；
  - 从任务详情接口中返回与之关联的事件列表或查询条件。

### 5.3 取消任务的可操作性

- 任务记录中应包含足够的信息，以便执行停止操作：
  - systemd transient unit 名称；
  - 或通过约定命名规则（例如 `webhook-task-<suffix>`）由 `task_id` 反推出 unit 名称。
- 对本身不可取消的瞬时任务：
  - 前端不展示停止按钮；
  - 状态直接从 `pending` / `running` 过渡到终态。

### 5.4 保留与清理

- 后端为 Task 相关表（`tasks` / `task_units` / `task_logs`）实现统一的保留策略：
  - 默认保留时长基于 `DEFAULT_STATE_RETENTION_SECS`（当前为 86400 秒，约 24 小时）；
  - 可通过环境变量 `PODUP_TASK_RETENTION_SECS`（单位：秒）覆盖，未配置时回退到 `DEFAULT_STATE_RETENTION_SECS`。
- 清理策略：
  - 仅清理已经处于终态的任务：`status IN ('succeeded','failed','cancelled','skipped')`；
  - 以 `finished_at` 为基准，要求 `finished_at IS NOT NULL` 且 `finished_at < now - retention`；
  - 删除发生在 `tasks` 表，依赖外键 `ON DELETE CASCADE` 自动删除对应的 `task_units` 与 `task_logs`。
- 触发入口：
  - CLI：`pod-upgrade-trigger prune-state` 每次执行时会在 state 清理之后尝试清理旧任务；
  - API：`POST /api/prune-state` 成功执行时同样会触发一次 Task 清理；
  - 其他 Task 执行路径（webhook / scheduler / manual 等）不会隐式触发清理，避免高频请求带来额外负载。
- dry-run 行为：
  - `prune-state --dry-run` 和 `POST /api/prune-state` 带 `dry_run=true` 时，Task 部分只做计数查询，不删除记录；
  - 结果通过 CLI 输出、`/api/prune-state` 响应字段 `tasks_removed` 以及系统事件 `cli-prune-state` / `prune-state-api` 的元数据暴露，便于运维审计。
- 列表默认展示最近一段时间数据，历史任务通过翻页访问；超出保留期的旧任务会在上述清理入口触发后被逐步回收。

---

## 六、前端设计与交互（页面模式 + 抽屉模式）

### 6.1 页面模式

#### 6.1.1 导航与路由

- 新增“任务”页面，建议路由为 `/tasks`：
  - 在侧边栏增加入口，位置可与 Events / Webhooks 相邻；
  - 可选：在菜单项右侧显示 running 任务数量徽章。

#### 6.1.2 布局

- 页面模式作为主视图，包含：
  - 顶部过滤区（条件输入 + 状态筛选 + 类型筛选 + unit 搜索）；
  - 任务列表表格：
    - 列包括：任务类型、状态、关联 unit 概要、触发来源、起始时间、耗时、简要摘要；
  - 分页控件。
- 详情展示方式：
  - 页面模式可采用“列表 + 详情右侧面板”或“跳转至 `/tasks/:taskId` 独立详情页”；
  - 无论采用哪种实现，功能上需要支持完整详情 + 日志视图（与抽屉模式一致）。

#### 6.1.3 交互

- 点击任务行：
  - 在页面模式下，一般跳转至详情页或打开右侧详情区；
  - 详情视图中提供：
    - 状态概览卡片；
    - unit 维度状态列表；
    - 日志时间线；
    - 停止按钮（仅 running 且用户具备权限时）。

---

### 6.2 抽屉模式

#### 6.2.1 抽屉定位与出场方式

- 抽屉从页面右侧滑出：
  - 桌面端：宽度约为视口宽度的 40–50%，高度覆盖全高；
  - 移动端：以全屏覆盖或自底部弹出为主（实现时可根据现有组件库能力调整）。
- 抽屉出现时：
  - 背景主页面保持可见，必要时可加半透明遮罩；
  - 通过显著的标题和关闭按钮表明当前查看的是具体任务。

#### 6.2.2 抽屉内容与信息密度

- 抽屉模式为全功能视图，只是布局更紧凑：
  - 顶部区域：
    - 任务类型 + 状态标签；
    - 起止时间、耗时；
    - 关联 unit 简要汇总（例如 “3 units · 2 ok / 1 failed”）。
  - 主体区域：
    - 可折叠的 unit 列表（点击展开 unit 详情与错误信息）；
    - 日志时间线（可默认展示最近若干条，支持展开“查看全部”）。
  - 底部操作条：
    - “停止任务”按钮（如适用）；
    - “在任务页面中打开”链接（跳转到 `/tasks/:taskId`）。
- 抽屉中所有功能应与页面模式一致：
  - 支持刷新任务状态与日志；
  - 支持停止任务；
  - 支持查看完整元数据（通过折叠面板或 JSON 视图）。

#### 6.2.3 抽屉的自动展开逻辑

- 在启动“长时间任务”时，前端应自动弹出抽屉，切换到新建任务的详情视图：
  - 典型场景：
    - 手动触发多 unit 更新（Maintenance / Manual 页面发起）；
    - 后续可能增加的维护任务（大规模 prune、校验等）。
  - 前端获得新任务信息后：
    - 通过 API 返回的 `task_id` / `long_running` 字段判断是否自动展开；
    - 创建任务成功时立即打开右侧抽屉，开始轮询该任务详情与日志。
- “长时间任务”的判定方式：
  - 推荐由后端在任务模型中显式给出：
    - 字段示例：`is_long_running: bool` 或 `expected_duration_secs: Option<u64>`；
  - 若后端暂未提供该字段：
    - 可以约定：由前端将“手工触发 + 关联多个 unit”的任务视为长时间任务，总是自动展开抽屉；
    - 后续再演进为基于后端标记的精确判定。

#### 6.2.4 抽屉打开与关闭行为

- 打开：
  - 用户显式点击任务列表中的“查看详情”按钮；
  - 或任务创建成功且满足“长时间任务”判定，即自动打开；
  - 从其他页面（例如 Manual 页面）启动任务时，不强制导航离开当前页面，而是优先使用抽屉。
- 关闭：
  - 点击抽屉右上角关闭按钮；
  - 点击遮罩区域（如设计需要）；
  - 关闭仅影响前端视图，不会停止任务本身。
- 抽屉关闭后：
  - 若当前页面为 `/tasks`：
    - 保持任务列表位置与筛选条件不变；
  - 若当前页面为非任务页面（如 Manual）：
    - 用户可继续进行其他操作；如再次触发新任务，抽屉可以针对新任务重新打开。

---

## 七、交互细节与状态反馈

- 状态提示：
  - 列表 / 详情 / 抽屉在加载时展示 skeleton 或 loading 文案；
  - 操作成功 / 失败通过 toast 提示（沿用现有 Toast 组件）。
- 错误处理：
  - API 返回错误时：
    - 401：按现有逻辑跳转 Unauthorized 页面或弹出错误 toast；
    - 4xx / 5xx：在抽屉 / 页面内展示清晰错误信息，并提供重试按钮。
- 轮询策略：
  - 列表与详情轮询的间隔与取消策略需要统一；
  - 当前 tab / 页面不可见时可降低轮询频率或暂停。

---

## 八、开放问题

- 停止语义：
  - 对哪些任务类型暴露“强制停止”按钮需要细化策略：
    - 例如对自动更新服务本身（auto-update unit）是否允许直接在 UI 中强制停止；
    - 是否需要为某些关键系统任务只保留“优雅停止”，隐藏强制停止入口。
- 保留策略：
  - 任务和日志保留时间是否需要在 Settings 页面中提供可配置项。

---

## 九、后端 Task API 合约草案

本节根据前端 `domain/tasks.ts` 与 MSW Mock 行为，整理出后端推荐采用的 Task 实体与 API 形状，作为后端实现的参考合约。

### 9.1 Task 实体结构

Task 对象（列表与详情中的核心单元）建议包含：

- `id: number`：内部自增主键，用于排序与调试；
- `task_id: string`：公开任务 ID，用于 URL 与 API 路由（`/api/tasks/:id` 中的 `:id`）；
- `kind: "manual" | "github-webhook" | "scheduler" | "maintenance" | "internal" | "other"`：任务类型；
- `status: "pending" | "running" | "succeeded" | "failed" | "cancelled" | "skipped"`：任务当前状态；
- `created_at: number`：创建时间，Unix 秒；
- `started_at?: number | null`：实际开始执行时间；
- `finished_at?: number | null`：到达终态时间；
- `updated_at?: number | null`：最近一次状态更新时间；
- `summary?: string | null`：用于列表展示的简短摘要，如 “3/5 units succeeded”；
- `trigger: { ... }`：触发元数据，字段建议为：
  - `source: "manual" | "webhook" | "scheduler" | "maintenance" | "cli" | "system"`；
  - `request_id?: string | null`：对应 `event_log.request_id`；
  - `path?: string | null`：来源 HTTP 路径或 CLI 命令标识；
  - `caller?: string | null`：手工触发时的 caller 信息；
  - `reason?: string | null`：手工/调度任务的理由说明；
  - `scheduler_iteration?: number | null`：由 scheduler 触发时的 iteration 序号；
- `units: TaskUnitSummary[]`：unit 维度摘要列表：
  - `unit: string`：systemd unit 名，例如 `svc-alpha.service`；
  - `slug?: string`：短标识（如 `svc-alpha`），用于 UI；
  - `display_name?: string`：可选的人类可读名称；
  - `status: TaskStatus`：与 Task 状态词汇一致；
  - `phase?: "queued" | "pulling-image" | "restarting" | "waiting" | "verifying" | "done"`：可选阶段提示，仅用于 UX；
  - `started_at?: number | null`、`finished_at?: number | null`、`duration_ms?: number | null`；
  - `message?: string | null`：简要说明；
  - `error?: string | null`：失败或中断时的错误字符串；
- `unit_counts: { total_units, succeeded, failed, cancelled, running, pending, skipped }`：unit 数量统计，用于列表摘要；
- `can_stop: boolean`：是否展示“停止任务”按钮；
- `can_force_stop: boolean`：是否展示“强制停止”按钮；
- `can_retry: boolean`：是否允许从该任务创建重试任务；
- `is_long_running?: boolean`：是否视为“长耗时任务”，前端可据此默认打开抽屉；
- `retry_of?: string | null`：若为重试任务，则指向原任务的 `task_id`。

### 9.2 Task 日志实体

详情端点在 Task 对象基础上附带日志数组：

- `TaskLogEntry`：
  - `id: number`：日志在任务内部的序号；
  - `ts: number`：事件时间（Unix 秒）；
  - `level: "info" | "warning" | "error"`：日志等级；
  - `action: string`：高层动作名，如 `task-created`、`image-pull`、`restart-unit`；
  - `status: TaskStatus`：该步骤对应的状态；
  - `summary: string`：用于时间线展示的短描述；
  - `unit?: string | null`：可选的关联 unit 名；
  - `meta?: unknown`：原始元数据，供 JSON 查看器使用。

详情响应形状：

```jsonc
{
  // Task 全量字段
  "id": 1,
  "task_id": "tsk_xxxxx",
  "kind": "manual",
  "status": "running",
  // ...
  "logs": [
    {
      "id": 1,
      "ts": 1700000000,
      "level": "info",
      "action": "task-created",
      "status": "running",
      "summary": "Manual task accepted from UI",
      "unit": null,
      "meta": { "caller": "ops" }
    }
  ]
}
```

### 9.3 API 端点与请求/响应

#### 9.3.1 `GET /api/tasks`

- 查询参数：
  - `page: number`：页码，从 1 开始；
  - `per_page: number`（或 `limit`）：每页条数，前端默认 20；
  - `status?: TaskStatus`：按任务状态过滤；
  - `kind?: TaskKind`（别名 `type`）：按任务类型过滤；
  - `unit?: string`（别名 `unit_query`）：按 unit/slug/display_name 模糊匹配。
- 返回体（与前端 `TasksListResponse` 对齐）：

```jsonc
{
  "tasks": [ /* Task[]，字段如上 */ ],
  "total": 42,
  "page": 1,
  "page_size": 20,
  "has_next": true
}
```

#### 9.3.2 `GET /api/tasks/:id`

- 路由参数：
  - `:id` 为 `task_id`（字符串），而非内部自增 `id`。
- 返回体：
  - `TaskDetailResponse = Task & { logs: TaskLogEntry[] }`，如上所述。

#### 9.3.3 `POST /api/tasks`

- 用途：由前端在 Manual/Maintenance 等页面创建长耗时任务。
- 请求体（与 mock `CreateTaskBody` 对齐）：

```jsonc
{
  "kind": "manual",              // 可选，默认 manual
  "source": "manual",            // 可选，默认 manual
  "units": ["svc-alpha.service"],// 关联 unit 列表，至少一个
  "caller": "ops-nightly",       // 可选
  "reason": "nightly rollout",   // 可选
  "path": "/api/manual/trigger", // 可选，来源路径
  "is_long_running": true        // 可选，默认为 true
}
```

- 响应体（轻量确认结构）：

```jsonc
{
  "task_id": "tsk_xxxxx",
  "is_long_running": true,
  "kind": "manual",
  "status": "running"
}
```

前端收到 `task_id` 后会自动打开抽屉并开始轮询 `/api/tasks/:id`。

#### 9.3.4 `POST /api/tasks/:id/stop`

- 语义：优雅停止任务（若仍在运行）。
- 行为建议：
  - 若任务为 `running`：
    - 后端尝试优雅停止，对应 systemd unit 可发送 SIGTERM 或等价操作；
    - 将任务状态更新为 `cancelled`，填充 `finished_at` 与 `summary`；
    - 写入一条 `task-cancelled` 日志行；
  - 若任务已处于终态：
    - 不改变状态，仅追加一条 `task-stop-noop` 日志。
- 响应体：更新后的 `TaskDetailResponse`。
- 错误：
  - `404`：任务不存在；
  - `401`：未通过 ForwardAuth 管控。

#### 9.3.5 `POST /api/tasks/:id/force-stop`

- 语义：强制终止任务。
- 行为建议：
  - 对 `running` 任务执行更强硬的终止动作（例如 `systemctl stop` 或等价信号），并将状态更新为 `failed`，追加 `task-force-killed` 日志；
  - 对终态任务只追加 `task-force-stop-noop` 日志。
- 响应体：更新后的 `TaskDetailResponse`。

#### 9.3.6 `POST /api/tasks/:id/retry`

- 语义：从终态任务创建重试任务。
- 行为建议：
  - 仅允许在原任务状态为 `succeeded` / `failed` / `cancelled` / `skipped` 时调用；
  - 新建一个 Task 记录：
    - `retry_of` 指向原任务 `task_id`；
    - 状态初始为 `pending` 或 `running`；
    - `units` 由原任务复制，但所有 unit 状态重置为 `pending`；
  - 在原任务的日志中追加 `task-retried` 记录。
- 返回体：新建任务的 `TaskDetailResponse`。
- 错误：
  - `409`：当原任务处于 `running` / `pending` 时拒绝重试；
  - `404`：任务不存在。

### 9.4 与 `event_log` 的关联关系

- 建议在 `event_log` 表中增加任务维度：
  - 方案一（推荐）：在表结构中增加 `task_id` 字段，指向 Task 表的业务主键；
  - 方案二：继续沿用 `meta` JSON，将 `task_id` 写入 `meta.task_id`。
- 关联原则：
  - 对于由 Task 驱动的 HTTP/CLI 操作，应在 `event_log` 中记录相同的 `task_id`；
  - Task 详情接口可返回：
    - 直接嵌入日志时间线（如 TaskLogEntry）；
    - 或提供查询条件（如 `request_id`/`task_id`），由前端跳转到 Events 页面进行深度分析。

在当前实现中，采用了“结构化列 + 元数据冗余”方案：

- `event_log` 表中增加了可空列 `task_id`，并在 Task 相关的 HTTP/CLI 操作中，将任务 ID 同时写入 `event_log.task_id` 与 `meta.task_id`，便于按任务维度查询与兼容旧数据；
- `/api/events` 支持可选查询参数 `task_id`，例如：`/api/events?task_id=tsk_xxx` 只返回该任务相关的事件记录；
- Task 详情接口（`GET /api/tasks/:id`）在原有字段基础上增加 `events_hint` 字段，形如：
  - `"events_hint": { "task_id": "tsk_xxx" }`，
  前端可以基于此构造跳转到 Events 视图的查询参数。

在当前实现中：

- HTTP 入口：
  - GitHub Webhook、手动触发 API（`/api/manual/*`）、`/auto-update`、scheduler loop 以及 `POST /api/prune-state` 都会创建 Task，并由 `run-task <task_id>` 统一执行底层的 `systemctl`/`podman` 命令；
  - `/api/prune-state` 在同步返回清理结果的同时，也会创建一个 `kind = "maintenance"` 的 Task 用于在任务面板中追踪本次清理。
- CLI 入口：
  - `trigger-units` / `trigger-all` 在非 `--dry-run` 模式下，会创建 `kind = "manual"`、`source = "cli"` 的 Task，并在同一进程内通过 `run-task <task_id>` 执行，CLI 输出由 Task 执行结果反推；
  - `prune-state` 会创建 `kind = "maintenance"`、`source = "cli"` 的 Task，并通过统一的维护任务执行逻辑完成清理。

### 9.5 状态机约定（建议）

- 任务级状态迁移：
  - `pending -> running -> {succeeded, failed, cancelled, skipped}`；
  - `pending -> cancelled`：任务尚未开始就被取消；
  - 终态一旦写入不得回退，仅允许补充 `summary` 与日志。
- unit 级状态迁移遵循相同词汇，但允许更细粒度的 `phase`：
  - 典型路径：`queued -> pulling-image -> restarting -> done`（成功）；
  - 或：`queued -> verifying -> failed`（失败）。

以上合约在不限制具体持久化实现的前提下，为前后端协同提供统一参考；后端实现时只要满足字段与语义约束，即可与当前 Tasks 页面与 Mock 行为保持一致。
