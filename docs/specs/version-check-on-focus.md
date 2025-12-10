# 聚焦触发的版本检查与新版本提示设计

## 背景

`pod-upgrade-trigger` 目前已经具备自更新能力，通过脚本和调度线程定期访问 GitHub Releases 获取最新版本并拉取二进制：

- 后端主程序在启动 `http-server` 时，根据环境变量 `PODUP_SELF_UPDATE_COMMAND`、`PODUP_SELF_UPDATE_CRON` 启动自更新调度线程；
- 自更新执行器脚本 `scripts/self-update-runner.sh` 调用 `scripts/update-pod-upgrade-trigger-from-release.sh`；
- 更新脚本通过 GitHub API `releases/latest` 获取最新 release tag，下载对应二进制和校验文件，并在非 dry-run 模式下替换现有二进制；
- 每次执行后生成 JSON 报告文件，由主程序周期性导入到 SQLite `tasks` 表，在 UI 的 `/tasks` 中以 kind=`self-update` / type=`self-update-run` 呈现。

现有 Web UI 能够从 `/api/settings` 读取当前运行的包版本（`version.package = env!("CARGO_PKG_VERSION")`）以及构建时间，但不会主动提示“有新版本可用”。同时，自更新链路目前由脚本自行调用 GitHub API 决定更新目标版本，主程序并未作为版本信息的统一“真相源”。

本设计希望：

- 给 UI 提供一个统一版本检查入口，用于在合适的频率下提示“有新版本可用”；
- 逐步将“获取最新版本信息 + 比较是否需要更新”的职责收归主程序，以便后续重整自更新链路，让脚本只负责“更新到指定版本”。

## 目标

1. 提供一个后端 HTTP 接口，使前端可以获取以下信息：
   - 当前运行版本信息（至少包含 `package` 版本号，可选 release tag）；
   - GitHub 上最新发布版本信息（latest release tag）；
   - 是否存在新版本（在可比较的前提下给出布尔判断）。
2. 在 Web UI 中实现“聚焦触发 + 冷却时间控制”的版本检查机制：
   - 页面重新获得焦点时，若距离上次检查时间已满 1 小时，则触发版本检查请求；
   - 若距离上次检查不足 1 小时，本次聚焦不触发检查，不访问后端并保持现有提示状态。
3. 在顶栏 `Pod Upgrade Trigger` 标题右侧显示“有新版本 vX.Y.Z 可用”的提示，点击可跳转到对应 GitHub Release 页。
4. 设计上为后续“主程序负责决定是否更新，再调用脚本更新到特定版本”预留扩展点，但不在本次变更中修改脚本行为。

## 范围与非目标

### 范围

- 后端：
  - 在主程序中实现统一的“版本信息获取 + 最新 Release 查询 + 版本比较”逻辑；
  - 新增一个专用版本检查 API（`GET /api/version/check`），返回当前版本与最新版本信息；
  - 定义清晰的错误行为和日志输出策略。
- 前端：
  - 新增版本检查 Hook，监听页面聚焦事件并实现“自上次检查已过 1 小时才触发” 的节流逻辑；
  - 在顶栏标题右侧展示新版本提示，包括版本号和跳转链接。

### 非目标

- 不在本次变更中：
  - 修改自更新脚本 `scripts/update-pod-upgrade-trigger-from-release.sh` 的行为；
  - 修改内建自更新调度线程（scheduler）的具体运行逻辑；
  - 引入 UI 内“一键触发自更新”的操作；
  - 定义多发布通道（beta/stable 等）的复杂版本策略。

上述内容可在后续 PR 中基于本设计演进。

## 后端设计

### 当前行为概览

- `/api/settings`：
  - 返回当前进程的包版本 `version.package = env!("CARGO_PKG_VERSION")` 和构建时间 `version.build_timestamp`；
  - 不访问 GitHub，也不判断是否存在新版本。
- 自更新链路：
  - `start_self_update_scheduler()` 根据环境变量配置，启动后台线程，周期性执行 `PODUP_SELF_UPDATE_COMMAND`；
  - 默认执行器脚本 `scripts/self-update-runner.sh` 调用 `scripts/update-pod-upgrade-trigger-from-release.sh`；
  - 更新脚本内部通过 `curl` 调用 GitHub `releases/latest` 获取 `tag_name` 作为目标版本，并下载/校验/替换二进制。

这意味着当前“最新版本”的判断完全由脚本负责，主程序只在任务导入和展示层面感知到自更新结果。

### 目标架构：主程序作为版本真相源

目标架构中，“获取最新版本信息 + 判断是否需要更新”的职责由主程序统一承担，具体为：

- 主程序内部提供：
  - 获取当前运行版本信息的能力；
  - 调用 GitHub API 获取最新 release 信息的能力；
  - 以统一规则比较当前版本与最新 release 的能力。
- 对外暴露：
  - `GET /api/version/check` 用于 UI 和外部系统获取版本状态；
  - 后续（不在本 PR 内）自更新调度线程可以复用同一逻辑，仅在确认有更新时才调用脚本，并将“目标版本 tag”作为参数传入。

### 内部数据结构与比较逻辑

在 Rust 侧抽象出三个核心结构：

```rust
struct CurrentVersion {
    package: String,             // 例如 "0.1.0"
    release_tag: Option<String>, // 例如 "v0.1.0"，可选
}

struct LatestRelease {
    release_tag: String,         // 例如 "v0.2.0"
    published_at: Option<String> // GitHub 的 published_at 字段，字符串格式，供展示使用
}

struct VersionComparison {
    current: CurrentVersion,
    latest: LatestRelease,
    has_update: Option<bool>,    // None 表示无法可靠比较
    checked_at: i64,             // Unix 秒级时间戳
    reason: String,              // "semver" / "tag-only" / "uncomparable" 等比较策略说明
}
```

获取当前版本：

- `CurrentVersion`：
  - `package`：使用 `env!("CARGO_PKG_VERSION")`；
  - `release_tag`：预留从环境变量（例如未来的 `PODUP_RELEASE_TAG`）读取的能力；现阶段可以为空或与 `package` 映射规则保持简单。

获取最新 release：

- `LatestRelease`：
  - 通过 HTTP 客户端（例如 `reqwest` 或其他轻量实现）调用 GitHub API：  
    `https://api.github.com/repos/ivanli-cn/pod-upgrade-trigger/releases/latest`；
  - 设置：
    - 合理的 `User-Agent`（可带上应用名与版本）；
    - 短超时（例如数秒级）；
    - 必要时支持 GitHub 令牌（留作扩展，初版可匿名）。
  - 解析返回 JSON 中的：
    - `tag_name` → `latest.release_tag`；
    - `published_at` → `latest.published_at`（可选，用于展示）。

版本比较策略：

- 主要面向 semver 风格版本号，兼容带前导 `v` 的 tag：
  1. 去掉 `current.package` 与 `latest.release_tag` 中的前导 `v` / `V`；
  2. 尝试按 semver 解析，如 `(major, minor, patch[, pre-release])`；
  3. 若两者都能解析，进行标准 semver 比较：
     - `latest > current` → `has_update = Some(true)`；
     - `latest == current` → `has_update = Some(false)`；
     - `latest < current` → `has_update = Some(false)`（允许主程序版本领先于 latest 的情况）。
  4. 若任一方无法解析：
     - `has_update = None`；
     - `reason` 标记为 `"uncomparable"`；
     - 前端可以只显示当前/最新版本字符串，不强行提示“有新版本”。

错误与降级：

- 调用 GitHub 或解析发生错误时：
  - 记录详细日志（包括 HTTP 状态码、错误类别等）；
  - 对外 API 返回合适的错误状态码（例如 502/503），由前端视为“版本信息暂不可用”，避免影响其他管理操作。

### 新 HTTP API：`GET /api/version/check`

路径与方法：

- `GET /api/version/check`

鉴权：

- 与现有 `/api/settings` 等管理接口保持一致，由前置的 ForwardAuth / 反向代理负责访问控制；
- API 本身不携带敏感信息，仅返回版本号和公开 release tag。

成功响应（示例）：

```json
{
  "current": {
    "package": "0.1.0",
    "release_tag": "v0.1.0"
  },
  "latest": {
    "release_tag": "v0.2.0",
    "published_at": "2025-02-01T11:22:33Z"
  },
  "has_update": true,
  "checked_at": 1731234567,
  "compare_reason": "semver"
}
```

字段说明：

- `current.package`：当前运行的包版本；
- `current.release_tag`：当前版本对应的 release tag（若可用）；
- `latest.release_tag`：GitHub 最新 release 的 tag；
- `latest.published_at`：最新 release 的发布时间（可用于显示附加信息）；
- `has_update`：
  - `true`：可以安全认为存在新版本；
  - `false`：可以安全认为当前不落后于 latest；
  - `null`：无法可靠比较，仅保证返回原始字符串；
- `checked_at`：主程序执行版本检查的时间；
- `compare_reason`：比较策略说明，用于调试与日志分析。

错误响应：

- GitHub API 不可用 / 超时 / 解析失败：
  - 返回 5xx（例如 502 Bad Gateway 或 503 Service Unavailable），并在 body 中包含简要错误信息；
  - 不阻断其他 API 的正常使用。

### 与自更新链路的关系

- 现状：
  - 自更新脚本内部自行调用 GitHub `releases/latest`，主程序并不参与“是否需要更新”的决策；
  - 脚本没有显式“更新到指定版本”的入口。
- 目标架构（后续演进方向）：
  - 自更新调度线程：
    - 使用与 `/api/version/check` 相同的内部版本检查逻辑；
    - 当 `has_update == Some(true)` 时才调用自更新脚本；
    - 调用脚本时显式传入目标 release tag，脚本不再自行访问 GitHub 决定更新版本。
  - 自更新脚本精简为“按指定版本更新 + 生成执行报告”的职责。
- 本次改动：
  - 不修改脚本行为，仅在主程序中建立版本检查的“单一真相源”；
  - 将版本比较逻辑封装为可复用模块，为后续重构自更新链路做好准备。

## 前端设计

### 状态模型与 Hook

新增 Hook：`useVersionCheck`，负责管理版本状态与检查节流逻辑。

状态结构（概念上）：

```ts
type VersionInfo = {
  current?: { package?: string; releaseTag?: string };
  latest?: { releaseTag?: string };
  hasUpdate?: boolean | null;
  lastCheckedAt?: number; // ms since epoch
  loading: boolean;
  error?: string;
}
```

职责：

- 读取与维护版本检查状态；
- 监听窗口/页面可见性变化；
- 控制检查频率：只有在“距离上次检查时间 ≥ 1 小时”且页面重新获得焦点时才发起新请求；
- 将 `lastCheckedAt` 写入 `localStorage`，在同一浏览器 profile 的多个标签页间共享冷却窗口。

### 触发机制与节流策略

事件监听：

- 在 Hook 挂载时：
  - 监听 `window` 的 `focus` 事件；
  - 可选：监听 `document.visibilitychange`，仅当 `document.visibilityState === 'visible'` 时才考虑检查。

节流逻辑：

- `localStorage` key 设计：
  - 例如：
    - `podup_version_last_check`：存储上次检查时间戳（ms）；
    - `podup_version_latest_tag`：存储最近一次成功检查到的 latest tag（可选，用于初始化提示）。
- 聚焦时逻辑：
  1. 读取 `lastCheckedAt`；
  2. 若不存在或 `Date.now() - lastCheckedAt >= 3600_000`：
     - 调用 `GET /api/version/check`；
     - 将返回结果写入内存状态；
     - 将当前时间写回 `localStorage`；
  3. 若 `< 3600_000`：
     - 本次不发请求，不修改版本相关状态。

错误处理：

- 后端返回非 2xx 或解析失败时：
  - 将错误信息记录在 `error` 字段；
  - 控制台日志打印详细信息；
  - 不弹出干扰性的 toast，避免打扰主要运维流程；
  - 保留上次成功检查的版本提示（若存在）。

### 顶栏 UI 展示

展示位置：

- 在 `TopStatusBar`（`web/src/App.tsx`）中，标题 “Pod Upgrade Trigger” 右侧追加版本状态区域。

展示规则（建议）：

- 当有可靠的更新信息时：
  - `hasUpdate === true` 且存在 `latest.release_tag`：
    - 显示一个醒目的 badge，例如：
      - 文案：`新版本 v0.2.0 可用`
      - 样式：使用 DaisyUI badge（如 `badge badge-warning` 或 `badge badge-accent`）；
      - 点击跳转到对应 GitHub Release 页：  
        `https://github.com/ivanli-cn/pod-upgrade-trigger/releases/tag/<tag>`。
- 当明确无更新：
  - `hasUpdate === false`：
    - 可以选择不展示任何内容（保持顶栏简洁），或者展示一个低调的“最新版本”标记；
    - 初版建议不额外展示，避免信息噪音。
- 当无法比较或请求失败：
  - `hasUpdate === null` 或请求异常：
    - 不展示新版本提示；
    - 可在未来需要时通过 tooltip 或图标表达“版本信息暂不可用”。

### 多标签页与浏览器行为

- 在同一浏览器 profile 内，`localStorage` 共享：
  - 任意一个标签页在过去 1 小时内完成了版本检查，其他标签页聚焦时不会再发新请求；
  - 在不同浏览器或不同设备之间不共享冷却窗口，这是可接受的限制。

## 测试与验证

后端测试：

- 单元测试：
  - 针对版本比较函数，覆盖：
    - 标准 semver 场景（如 `0.1.0`, `0.2.0`）；
    - 带前导 `v` 的 tag（`v0.1.0` vs `0.1.0`）；
    - 无法解析的版本字符串，确认 `has_update = None`；
  - 针对 GitHub 响应解析函数，使用固定 JSON 样本验证 `tag_name` / `published_at` 的提取。
- 集成测试：
  - 在测试环境中对 `/api/version/check` 进行调用：
    - 模拟成功返回时的 JSON 结构；
    - 模拟 GitHub 不可用时，验证 API 返回错误状态并日志记录。

前端测试：

- Hook 行为测试：
  - 使用 jest/rtl 或 Playwright，对 `useVersionCheck` 在不同时间间隔下的行为进行验证；
  - 模拟 `focus` 和 `visibilitychange` 事件，确认冷却逻辑正确生效。
- UI 端到端测试（可选）：
  - 在 mock 模式下模拟 `/api/version/check` 返回“有更新”的响应；
  - 确认顶栏出现新版本提示，且点击跳转的链接正确。

手工验证：

- 在真实环境中：
  - 部署一个旧版本的 `pod-upgrade-trigger`，同时 GitHub 上存在更新的 release；
  - 打开 UI，等待首次聚焦触发版本检查；
  - 确认顶栏显示“新版本 vX.Y.Z 可用”；
  - 切换到其他标签页后再切回，在 1 小时内不重复发起检查；
  - 1 小时后再次切回，确认会重新检查并更新提示。

## 演进方向（后续工作）

本设计为后续自更新链路重构预留扩展点，推荐的演进步骤包括：

1. 修改自更新调度线程，使其在每次调度周期中先调用内部版本检查逻辑，仅在确认为“存在新版本”时才触发脚本；
2. 调整自更新脚本接口，改为接受“目标 release tag”参数，不再自行访问 GitHub 决定更新版本；
3. 在 UI 中补充“最近自更新结果”和“下一次检查时间”等辅助信息，将版本提示与 `tasks` 中的 self-update 任务更好地关联起来。

这些步骤可以在当前版本检查接口稳定后，以独立 PR 的形式逐步落实。 

