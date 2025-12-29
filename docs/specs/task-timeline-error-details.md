# 任务详情「日志时间线」错误详情展示与诊断信息补强

## 背景

当前 Web UI 的任务详情页在渲染「日志时间线」时，主要展示 `TaskLogEntry.summary`；仅对少数特殊场景（例如 command 型日志、task-dispatch-failed）额外展示 `meta` 中的内容。

在 Manual service 等任务失败时，后端 `GET /api/tasks/:task_id` 的 `logs[].meta` 往往包含可用于排障的关键信息（例如 `result_message` 的 systemd 错误文本），但 UI 未显式展示，导致用户只能看到泛化文案（如 “Manual service task failed”），排障成本较高。

进一步地，即便 UI 展示了 `result_message`，某些 systemd 类失败的根因仍需要查看 `systemctl status` / `journalctl -u` 等诊断信息。当前任务日志对这类“失败后诊断输出”缺少采集规范，使得 UI 的信息量仍可能不如手动排障。

## 目标

1. 在任务详情页的「日志时间线」中，为每条日志记录展示关键 `meta` 字段，重点展示 `meta.result_message`（当存在时）。
2. 当 `meta.result_message` 过长时，默认折叠并支持「展开/收起」查看完整内容。
3. 当存在 `meta.unit` / `meta.image` / `meta.result_status` 时，以轻量的 key-value 方式展示，辅助定位问题。
4. 在缺少 `meta` 或缺少上述字段时，保持现有行为：继续展示 `summary` 作为兜底。
5. 补强后端对关键步骤的采集，使 UI 可获得不逊色于手动排障的“命令 + 输出 + 失败诊断”信息：
   - 外部命令执行：记录 `command/argv/exit/stdout/stderr`；
   - unit 操作失败后：可选采集 `systemctl status` 与 `journalctl` 摘要输出（受控、可裁剪）。

## 范围与非目标

### 范围

- 修改 Web UI 展示层：
  - 任务中心列表抽屉详情（`/tasks` -> drawer -> timeline）
  - 手动任务页面详情（`/manual` -> drawer/detail -> timeline）
- 新增前端侧的 meta 解析/归一化逻辑（容错 stringified JSON）。
- 新增时间线内的“错误详情/关键信息”渲染块与折叠交互。
- 修改后端任务日志采集（仅补强、保持兼容）：
  - 为 manual 相关任务的 unit 操作补齐 command 型日志元数据；
  - 在 unit 操作失败时，按配置追加“诊断日志”条目（best-effort）。

### 非目标

- 不修改后端 API 与数据库 schema。
- 不引入通用的 raw meta JSON Inspector（现有“导出 JSON”功能仍可作为深度排障手段）。
- 不改变现有时间线排序、状态徽章、command 输出块与 task-dispatch-failed 的展示语义。
- 不承诺在所有部署环境都能采集到 journal（例如容器内无 user journal/无权限时），此类采集为 best-effort，并在日志中可见失败原因。

## 现状说明（与问题关联）

- `TaskLogEntry.meta` 在前端类型为 `unknown`，目前仅通过 `isCommandMeta(meta)` 识别 command 型日志并展示 stdout/stderr。
- Manual service 失败日志（如 action=`manual-service-run`）的 `meta.result_message` 不符合 command meta 形态，因此不会出现在 UI 中。
- Manual/批量触发类任务的 unit 操作通常只记录摘要（例如 `result_message` 或 `UnitActionResult.message`），缺少可对齐手动排障的 command 输出与失败诊断（status/journal）采集。

## 设计

### A) 前端展示

#### 1) Meta 解析与归一化（前端）

新增一个轻量的 meta 提取 helper（或组件内部 helper），只做“读取已知字段”的白名单解析：

- 输入：`meta: unknown`
- 容错：
  - `meta` 为对象：直接读取 key。
  - `meta` 为 string：尝试 `JSON.parse`，成功后按对象处理，失败则视为无可用 meta。
- 输出（仅白名单字段）：

```ts
type TaskLogMetaHints = {
  unit?: string
  image?: string | null
  result_status?: string
  result_message?: string
}
```

说明：
- 只抽取上述字段，避免误展示大体积或不稳定的 meta 内容。
- `result_message` 仅作为文本展示（纯文本、换行保留），不渲染 HTML。

#### 2) 时间线渲染规则（UI）

对每条日志记录保持现有结构（action/status/unit/ts + summary），并追加“详细信息”区块：

1. **优先展示 `result_message`**
   - 当 `result_message` 为非空字符串时，在 summary 下方展示为多行文本（`whitespace-pre-wrap` + `break-words` 风格）。
2. **折叠策略**
   - 触发条件（择一即可）：
     - 行数 > 3（按 `\n` 分割）
     - 或字符数 > 200（避免单行过长）
   - 默认行为：
     - 超阈值：默认折叠，展示预览（例如前 3 行或前 N 字符）+ 「展开详情」
     - 未超阈值：默认展开（不显示按钮或显示「收起详情」可选）
3. **展示辅助 key-value**
   - 当存在 `unit/image/result_status` 任一字段时，以次级样式展示：
     - 示例：`unit · lobe-chat.service`、`result_status · failed`、`image · ghcr.io/...`
   - `image` 为 `null` 时跳过渲染。
4. **与既有特殊渲染的关系**
   - `task-dispatch-failed` 的专用提示仍保留；
   - command 型日志的“命令输出”折叠块仍保留；
   - 新增的 meta hints 仅在白名单字段存在时渲染，避免重复和噪音。

#### 3) 交互与状态管理

- 折叠状态以 `log.id` 为 key 存储在组件 state 中（例如 `expandedMetaDetails: Record<number, boolean>`）。
- 在日志列表刷新/轮询时：
  - 以 `log.id` 为稳定键，尽量保留用户已展开/收起的状态；
  - 对新增的日志项按“是否超阈值”决定默认展开/折叠。

### B) 后端采集补强（信息量对齐手动排障）

本节的目标是：让 UI 在“任务详情”中能直接回答 **做了什么 / 做得怎么样 / 为什么失败**，并尽量不逊色于手动排障的可见信息量。

#### 1) 统一外部命令采集规范（task_logs.meta）

对所有“外部命令型步骤”（podman/systemctl/journalctl 等）统一使用 command meta，并保持与前端 `isCommandMeta` 的识别兼容：

```json
{
  "type": "command",
  "command": "…",
  "argv": ["…"],
  "exit": "exit=…",
  "stdout": "…",
  "stderr": "…",
  "truncated_stdout": true,
  "truncated_stderr": true,

  "unit": "xxx.service",
  "image": "ghcr.io/…",
  "runner": "systemctl",
  "purpose": "restart|start|diagnose-status|diagnose-journal"
}
```

规范说明：
- `command/argv` 建议记录“用户可复现/可理解”的命令（优先使用 `systemctl --user …` 的形式），与现有任务日志展示习惯保持一致。
- 通过 `runner` 标注实际执行路径；如需保留真实调用细节，可附加 `runner_command`（不要求 UI 默认展示）。
- 输出长度继续受后端的 `COMMAND_OUTPUT_MAX_LEN` 截断保护（避免 UI/DB 被大输出拖垮）。
- 对同一阶段的高层摘要仍保留 `summary`（人类可读，列表/时间线概览继续使用）。

#### 2) Manual service：补齐 unit 操作的 command meta（重点）

当前 manual-service-run 失败信息主要体现在 `meta.result_message`，但缺少 command meta（command/argv/stdout/stderr），导致 UI 即便展示了 result_message，信息量仍可能不足。

补强要求：
- 在执行 unit start/restart 时，写入一条 **可识别为 command meta** 的日志条目，且包含 `exit/stderr`（有则包含 stdout）。
- 保持 `summary` 现有语义（成功/失败一句话），以保证概览一致。

建议落地：
- 将 `manual-service-run` 这条日志的 `meta` 升级为 command meta，并在 meta 中附带：
  - `unit` / `image`（若有）
  - `runner`（systemctl）
  - `purpose`（start/restart）
- `result_message` 可继续作为额外字段保留（兼容历史 UI 展示与导出 JSON）。

#### 3) Manual trigger（多 unit）：可定位到“哪个 unit 为什么失败”

manual-trigger-run 需要同时满足两类可读性：
- 总览：一个汇总条目（例如 `3/5 triggered, 2 failed`）；
- 细节：能看到每个失败 unit 的具体原因，且尽量具备命令输出信息量。

补强要求：
- 每个 unit 操作至少产出一条“可定位 unit 的日志”（包含 unit），失败时必须包含失败详情；
- 推荐为 unit 操作补齐 command meta（以便 UI 展示命令输出）。

建议落地：
- 保留 `manual-trigger-run` 汇总日志（用于概览）。
- 对每个 unit 的 start/restart 操作追加日志条目（action 可复用 `restart-unit` / `auto-update-start`，或统一为 `manual-trigger-unit-run`），meta 为 command meta，`unit` 字段必填。

#### 4) 失败诊断日志（best-effort，受控）

当 unit start/restart 失败时，按配置追加诊断日志条目，用于在 UI 内快速定位根因：

- `systemctl --user status <unit> --no-pager --full`
- `journalctl --user -u <unit> -n <N> --no-pager --output=short-precise`

原则：
- **仅在失败时执行**；
- **best-effort**：若命令不存在/无权限/执行失败，仍写入日志条目，meta 中记录 `error`；
- **默认安全**：journal 可能包含敏感信息，建议使用开关控制是否采集。

建议配置（示例，最终命名以实现为准）：
- `PODUP_TASK_DIAGNOSTICS=1`：启用失败诊断采集（默认 0/false）。
- `PODUP_TASK_DIAGNOSTICS_JOURNAL_LINES=100`：journal 行数上限（默认 100）。

#### 5) Unit 列表的错误摘要（task_units.error）

为了让主人无需展开时间线也能快速发现问题：
- 对单 unit 的 manual-service 失败：建议将错误摘要写入 `task_units.error`（例如 exit + stderr tail 或 `result_message`），`task_units.message` 保持高层 summary。
- 对多 unit 的 manual-trigger：维持现有 `task_units.message = UnitActionResult.message` 的行为，并在可行时同步写入 `error` 字段以便 UI 强化展示。

## 兼容性考虑

- 旧任务日志不包含 `meta.result_message`：行为不变，继续只显示 summary。
- `meta` 为非对象或不可解析字符串：忽略，不影响渲染稳定性。
- `meta` 字段未来扩展：因白名单解析，不会影响当前 UI。
- 补强后的 command meta 与既有 `isCommandMeta` 兼容：旧 UI 仍可按“命令输出”方式展示。
- 诊断日志为增量条目，且受开关控制：默认关闭时不改变现有任务的产出与展示。

## 测试计划

1. 后端单测/集成测试（Rust）
   - manual-service-run 失败：可在 task_logs 中读取到可展示的失败详情（result_message 或 command meta 中的 stderr/exit）；
   - manual-service-run（补强后）：存在 command meta（`type=command`），且输出受长度限制；
   - 失败诊断（开关开启）：会追加 status/journal 的日志条目；开关关闭则不追加；
   - manual-trigger（多 unit）：能定位到每个失败 unit 的失败原因（至少 exit/stderr 摘要）。
2. UI E2E（mock）
   - 构造包含 `meta.result_message` 的日志条目：断言可见错误文本与折叠/展开行为；
   - 构造包含 command meta 的日志条目：断言“命令输出”可展开且包含 stderr；
   - 构造诊断日志条目：断言可在时间线中查看到 status/journal 摘要输出。
3. 回归验证
   - 既有 command 输出与 task-dispatch-failed 展示不回归；
   - 无 meta 的历史日志无 UI 变化。

## 验收标准

1. 对于包含 `meta.result_message` 的失败日志，时间线中能直接看到具体错误信息（无需再手动请求 `/api/tasks/:task_id`）。
2. 长 `result_message` 默认折叠，可展开查看完整内容。
3. 不包含 `result_message` 的历史任务，至少保留现有 summary 展示，且不引入报错或布局破坏。
4. 开启诊断采集后，当 unit 操作失败，时间线中能看到一条或多条 best-effort 的诊断输出（status/journal），并且输出受长度限制。
