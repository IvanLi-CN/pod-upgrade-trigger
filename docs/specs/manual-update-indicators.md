# Services 页面更新提示（tag 内/外）设计

## 背景

当前 `/manual`（UI 标签：**Services**）页面主要提供两类能力：

- 列出可触发的 systemd unit（来自 `GET /api/manual/services`），并展示其默认镜像（来自 Quadlet / unit 定义中的 `Image=`）。
- 对单个服务或全部可部署服务发起部署（`POST /api/manual/services/<slug>` / `POST /api/manual/deploy`），后端执行 `podman pull <image>` + `systemctl/busctl restart|start <unit>`（auto-update excluded；缺少默认镜像的服务会被跳过）。

但 UI 缺少“是否有可更新内容”的提示。主人希望在服务列表项上看到两种不同语义的更新标记：

1. **tag 内有更新**：服务配置的 tag（例如 `:latest` 或 `:v1.2.3`）在 registry 上已经指向了新的镜像，但当前运行中的容器仍在使用旧镜像；此时点击“触发更新”（仍使用原 tag）应能更新到新镜像。
2. **tag 外有更新**：服务配置的 tag 没有变化（或已跟上），但 `:latest` 指向的镜像与当前 tag 指向的镜像不同；此时仅提示“有更高版本”，但不要求在本次方案中提供“改 tag 升级”的能力。

本设计只做“提示与判断”，不引入“固定 digest”或“切换版本”。

## 目标

1. 在 Services 的服务列表项上展示更新状态标记，并区分：
   - `tag_update_available`（tag 内有更新，可直接触发更新解决；UI 文案：**有新版本**）
   - `latest_ahead`（tag 外有更新，仅提示；UI 文案：**有更高版本**）
   - `up_to_date`（无更新）
   - `unknown`（无法判定）
2. 后端提供可靠的判定数据：基于“当前运行容器的镜像 digest”与“registry 上某 tag 的 manifest digest”对比。
3. 为性能与稳定性引入缓存与降级策略，避免 UI 打开即触发大规模外网请求。

## 范围与非目标

### 范围

- 后端：
  - 扩展 `GET /api/manual/services` 的返回结构，为每个服务附加 `update` 状态信息（字段新增且可选，保持向后兼容）。
  - 实现“unit -> running container digest”解析。
  - 实现“image tag -> remote digest”查询，并带 TTL 缓存（持久化到 SQLite）。
- 前端：
  - 在 `ManualPage` 的服务列表项中展示状态 badge/提示文案，并提供“刷新状态”（可选）入口。

### 非目标

- 不在本次变更中：
  - 允许 UI 选择版本、修改 Quadlet `Image=`、或固定到 digest；
  - 实现“完整 tag 列表/版本选择器”；
  - 改造手动触发任务的实时日志（另一个独立工作项）；
  - 引入新的外部依赖（例如强制要求 skopeo），但允许在可用时作为可选加速路径。

## 术语

- **configured image**：从 unit/Quadlet 解析出的 `Image=`（例如 `ghcr.io/acme/app:stable`）。
- **tag digest（remote）**：registry 上 `repo:tag` 当前指向的 manifest digest（例如 `sha256:...`）。
- **running digest（local）**：当前运行容器实际使用的镜像 digest（理想情况下可映射为仓库的 manifest digest）。
- **tag 内更新**：`running_digest != remote_tag_digest`。
- **tag 外更新**：在 `tag != latest` 且 `remote_tag_digest` 可得时，`remote_latest_digest != remote_tag_digest`。

## 关键用例 / 用户流程

1. 主人打开 `/manual`（Services）：
   - UI 调用 `GET /api/manual/services` 获取服务列表与 `update` 信息。
   - 列表项显示：
     - `tag_update_available`：显著提示“有新版本”，并暗示可直接触发更新。
     - `latest_ahead`：提示“有更高版本”（即 `latest` 通道与当前 tag 通道不同）。
     - `up_to_date`：提示“已是最新（对当前 tag）”。
     - `unknown`：提示“未知（缺少权限/容器未运行/查询失败）”。
2. 主人点击“刷新状态”（可选）：
   - UI 重新请求 `GET /api/manual/services?refresh=1` 强制刷新缓存并更新状态。

## 后端设计

### 总体思路

后端在 `GET /api/manual/services` 构建服务列表时，为每个 unit 计算更新状态：

1. 解析 configured image（repo + tag）。
2. 获取 running digest（通过 Podman 查询当前运行容器）。
3. 查询 remote digest：
   - `repo:<tag>` 的 digest
   - `repo:latest` 的 digest（仅当 `tag != latest` 时）
4. 依规则产出 `update.status` 与必要的可解释字段。

### API：扩展 `GET /api/manual/services`

在现有服务条目基础上新增字段（均为可选）：

```json
{
  "services": [
    {
      "slug": "svc-alpha",
      "unit": "svc-alpha.service",
      "display_name": "svc-alpha.service",
      "default_image": "ghcr.io/acme/svc-alpha:stable",
      "source": "manual|discovered",
      "is_auto_update": false,
      "update": {
        "status": "up_to_date|tag_update_available|latest_ahead|unknown",
        "tag": "stable",
        "running_digest": "sha256:...",
        "remote_tag_digest": "sha256:...",
        "remote_latest_digest": "sha256:...",
        "checked_at": 1731234567,
        "stale": false,
        "reason": "optional short reason"
      }
    }
  ]
}
```

说明：

- `checked_at/stale`：体现 remote digest 缓存的更新时间与是否过期，便于 UI 解释“这是缓存结果”。
- `reason`：用于 `unknown` 或降级路径时给出短原因（例如 `container-not-found` / `registry-auth-missing` / `remote-timeout`）。

查询参数：

- 沿用已有 `?refresh=1` 语义：当携带 `refresh` 时，强制刷新 discovery（现有行为）并同时强制刷新 remote digest 缓存（本设计新增行为）。

### running digest：unit -> container -> digest

优先走“可稳定映射”的 Podman 标签：

1. `podman ps -a --filter label=io.podman.systemd.unit=<unit> --format json`（或 `--filter label=PODMAN_SYSTEMD_UNIT=<unit>`）
2. 若匹配到多个容器：
   - 若存在 running 容器，优先 running；
   - 否则取最新创建的一个（或返回 unknown，取决于实现复杂度）。
3. 获取 digest 的实现路径（建议按可用性降级）：
   - A) `podman container inspect <id>`：若存在可直接读取的 image digest 字段（实现时确认具体字段名）。
   - B) 读取容器的 image ID / image name 后执行 `podman image inspect`，从 `RepoDigests` 中挑选与 configured repo 匹配的 digest。

若 unit 对应的是 pod/kube 等多容器实体，本次先返回 `unknown`（`reason=multi-container-unit`），避免误导；后续可扩展为“多个容器汇总状态”。

### remote digest：tag -> digest（带缓存）

#### 目标

给定 `registry/repo:tag`，获取其远端 manifest digest（不下载镜像层）。

#### 实现策略

推荐实现一个“digest resolver”抽象，按顺序尝试：

1. **OCI Distribution HEAD**（主路径）
   - 请求：`HEAD https://<registry>/v2/<repo>/manifests/<tag>`
   - 读取响应头中的 digest（常见为 `Docker-Content-Digest`）。
   - 处理 `401`：按 `WWW-Authenticate` 的 Bearer challenge 流程获取 token 后重试。
   - 鉴权来源（本次决策）：**复用 rootless 用户的 `~/.config/containers/auth.json`**。若无法读取或缺失对应 registry 的凭据，则返回 `unknown`（仅影响提示，不影响手动触发）。
2. **可选外部命令（加速/简化）**
   - 若环境已安装 `skopeo`，可用 `skopeo inspect docker://<image>` 获取 `Digest`，避免自行处理 bearer 流程。
   - 该路径为可选加速，不作为必需依赖。

#### 缓存设计（SQLite）

由于本项目采用“每个连接派生 server 子进程”的模型，进程内缓存无法跨请求复用，因此需要持久化缓存。

新增表（示例）：

- `registry_digest_cache(image TEXT PRIMARY KEY, digest TEXT, checked_at INTEGER, status TEXT, error TEXT)`

缓存规则：

- 默认 TTL：例如 10 分钟（可由环境变量配置）。
- `GET /api/manual/services`：
  - 若缓存未过期：直接使用缓存；
  - 若缓存过期且请求带 `refresh=1`：同步刷新（带短超时），刷新失败则沿用旧值并标记 `stale=true`；
  - 若缓存过期但未 refresh：可直接返回旧值（`stale=true`）或返回 unknown（取决于 UI 期望；推荐返回旧值以减少抖动）。

### 状态判定规则（核心）

对每个服务（configured tag 为 `tag`）：

1. 若无法取得 `running_digest` 或 `remote_tag_digest`：`status=unknown`
2. 否则若 `running_digest != remote_tag_digest`：`status=tag_update_available`
3. 否则若 `tag != latest` 且 `remote_latest_digest` 可得 且 `remote_latest_digest != remote_tag_digest`：
   - `status=latest_ahead`
4. 否则：`status=up_to_date`

备注：

- `latest_ahead` 的语义只表示“latest 与当前 tag 的 digest 不同”，不承诺“更高 semver”，避免误导。
- 本次判定策略只使用“digest 不同”作为更新信号，不引入语义化版本比较、发布时间比较等复杂逻辑。

## 前端设计（Services 页面）

### 展示

在每个服务行（ServiceRow）新增一个小型 badge 区域：

- `tag_update_available`：例如 “Update available (tag)” / “有新版本”，并显示当前服务的 tag（例如 `stable` / `v1.2.3`）。
- `latest_ahead`：例如 “Latest differs” / “有更高版本”，并显示目标 tag（`latest`）。
- `up_to_date`：例如 “Up to date”
- `unknown`：例如 “Unknown”

可选增强：

- hover/展开显示 `running_digest` 与 `checked_at`（仅短显示），用于排障。

### 刷新

在 Manual 页面顶部或服务列表标题处新增 “Refresh update status” 按钮：

- 触发 `GET /api/manual/services?refresh=1`
- UI 侧显示 loading，并在失败时 toast 提示。

## 兼容性与迁移

- API 变更为**字段新增**，旧版前端可忽略 `update` 字段。
- 需要新增 SQLite migration 创建缓存表；旧 DB 将自动迁移。
- 对没有 registry 凭据或 registry 不可达的环境：统一回退到 `unknown`，不影响手动触发功能。

## 风险点与待确认问题

1. **registry 鉴权策略（已确认）**：复用 rootless 用户的 `~/.config/containers/auth.json`。需要确认运维在生产主机上为运行用户正确配置该文件，并具备拉取/查询 manifest 的权限。
2. **digest 口径一致性**：本地运行容器能否稳定拿到“与 registry 对齐的 manifest digest”？多架构镜像可能出现 index digest 与 platform digest 不一致，需要明确比较口径。
3. **unit->container 映射可靠性（已确认方向）**：依赖标准 Quadlet + rootless 部署，优先通过 `io.podman.systemd.unit` / `PODMAN_SYSTEMD_UNIT` label 映射；若 label 缺失则降级为 `unknown` 并提示运维修正。
4. **性能**：服务数量较多时，首次 refresh 可能触发较多外网 HEAD；需要并发控制与总超时。

## 测试计划（建议）

- 后端单元测试：
  - image 解析（registry/repo/tag）覆盖常见输入与错误输入。
  - 状态判定规则（四态）覆盖。
- 集成测试：
  - 使用 `tests/mock-bin/podman` 扩展 mock 输出（或新增 mock 行为）以模拟：
    - unit label 映射到容器
    - container/image inspect 返回 digest
  - 以本地 HTTP server 模拟 registry HEAD（返回 digest / 401 challenge / timeout），验证缓存与 refresh 逻辑。
