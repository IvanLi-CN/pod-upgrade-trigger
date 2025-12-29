# Systemctl-only unit 操作与镜像更新核验（Tasks 时间线分步日志）

## 背景

当前任务时间线存在两个可读性/可验证性问题：

1. unit 的 `start/restart` 可能通过 D-Bus（例如 `busctl`）触发，stdout 会出现类似 `"/org/freedesktop/systemd1/job/..."` 的 object path。这对人类阅读几乎没有价值，且容易被误认为“异常日志”。
2. 多个步骤的输出被混入“最后一条总结日志”，导致时间线无法直观看清：执行顺序、每一步是否成功、以及“最终容器是否真的使用了期望镜像”。

本工作项的目标是：统一使用 `systemctl --user`，把关键步骤拆为独立日志，并在任务结束后提供可核验的“镜像版本三件套”。

## 目标

- **Systemctl-only**：所有 unit 操作仅使用 `systemctl --user start|restart`，不再走其它方案。
- **分步日志**：不同操作分别展示为独立时间线条目（pull / start|restart / health / verify / summary / diagnose）。
- **成功口径清晰**：定义并统一 `succeeded` / `failed` / `unknown(异常)` 的判定规则。
- **镜像可验证**：对“本次任务包含 image（执行了 image-pull）”的任务，结束后记录并展示：
  - 发现的最新镜像版本（remote）
  - 更新到的镜像版本（pulled）
  - 更新完成后容器实际使用的镜像版本（running）

## 范围与非目标

### 范围

- 覆盖所有会触发 unit start/restart 的任务路径（manual / webhook / auto-update 等）。
- 仅对“本次任务执行了 `image-pull`”的任务增加 `image-verify` 步骤。
- 当任务判定为 `failed` 时，**必须**追加必要的失败诊断步骤（用于事后确认与定位问题）。

### 非目标

- 不做持续健康监控（任务结束后服务未来是否崩溃不在本次保证范围内）。
- 不提供版本选择/切换 tag/固定 digest 的 UI 能力；仅做核验与日志留证。

## 术语与口径

- **image ref**：例如 `ghcr.io/acme/app:stable`（本次任务使用的镜像引用）。
- **platform**：运行环境平台三元组 `os/arch[/variant]`（用于从多架构 manifest list/index 选择正确的 manifest）。
- **remote_index_digest**：registry 上 `image ref` 指向的 *index/list* digest（可能也是单 manifest digest）。
- **remote_platform_digest**：针对本机 platform 解析出的 *platform manifest* digest（本次比较的主口径）。
- **pulled_digest**：本次 `podman pull` 后，本地镜像解析得到的 platform manifest digest（“更新到的版本”）。
- **running_digest**：unit 重启后，运行容器实际使用的 platform manifest digest（“实际在用的版本”）。

> 多架构镜像可能出现 `remote_index_digest != remote_platform_digest`，因此对比与核验以 `remote_platform_digest` 为准，同时保留 `remote_index_digest` 便于排障留证。

## 关键流程

### A) 带 image 的部署任务（执行了 image-pull）

1. `image-pull`：`podman pull <image ref>`
2. `restart-unit` / `start-unit`：`systemctl --user restart|start <unit>`
3. `unit-health-check`：`systemctl --user show <unit> ...`（判定必须 Healthy，否则失败）
4. `image-verify`：获取并记录 `remote_*_digest / pulled_digest / running_digest`，并进行一致性核验
5. 最终总结：仅 summary，不承载命令输出

### B) 仅重启类任务（未执行 image-pull）

不运行 `image-verify`；仍保持：

1. `restart-unit` / `start-unit`
2. `unit-health-check`
3. 最终总结

## 状态判定（TaskStatus）

### succeeded

- unit 操作 exit=0；且
- `unit-health-check` verdict = `Healthy`；且
- 若执行了 `image-verify`：digest 核验通过（详见下文）

### failed

满足任一即失败，并追加诊断步骤：

- `systemctl --user start|restart` exit≠0
- `unit-health-check` verdict ≠ `Healthy`（`Degraded/Unknown` 也一律视为失败）
- `image-verify` 可判定且确认不一致（例如 `running_digest != pulled_digest`，或 `remote_platform_digest` 存在但与本地不一致）
- `image-verify` 的本地关键信息缺失且无法给出可信解释（例如无法解析 running_digest 且容器确实应存在）

### unknown（异常）

用于表达“unit 操作与健康检查均成功，但无法确认镜像是否达到远端最新”的终止态：

- `image-verify` 中 **远端 digest 获取失败**（超时/鉴权缺失/registry 不可达/响应不符合预期等）

unknown 时必须在 `image-verify` 的 meta 中写明 `remote_error`（短错误码/摘要），并尽可能记录本地的 `pulled_digest/running_digest`，以便事后确认。

## 时间线日志与 meta 约定

### command 型日志（沿用现有 UI 展开逻辑）

对 `podman/systemctl/journalctl` 等外部命令步骤，统一使用 command meta：

- `type: "command"`
- `command/argv/exit/stdout/stderr`
- 额外字段（不影响 UI 兼容）：
  - `unit`（如适用）
  - `image`（如适用）
  - `purpose`（例如 `restart`/`start`/`diagnose-*`）

> 重要：最终总结日志 **禁止**携带 `type=command` 或 `command/stdout/stderr` 字段，避免 UI 把总结误渲染为“命令输出块”。

### image-verify 日志（新）

- action：`image-verify`
- status：`succeeded|failed|unknown`
- summary：一句话结论（例如 “Image verify: OK” / “Image verify: FAILED” / “Image verify: unavailable”）
- meta（结构化字段 + 人类可读摘要）：

```json
{
  "unit": "svc-alpha.service",
  "image": "ghcr.io/acme/svc-alpha:stable",
  "platform": { "os": "linux", "arch": "amd64", "variant": null },
  "remote_index_digest": "sha256:...",
  "remote_platform_digest": "sha256:...",
  "pulled_digest": "sha256:...",
  "running_digest": "sha256:...",
  "remote_error": null,
  "local_error": null,
  "result_status": "ok|failed|unknown",
  "result_message": "remote=… pulled=… running=…"
}
```

`result_message` 用于直接在时间线中可读展示（前端可白名单提取/折叠）。

## 后端概要设计

### 1) 统一 systemctl-only

- 所有 unit start/restart 逻辑统一走 `host_backend.systemctl_user(...)`。
- 禁止在 start/restart 路径上调用 `busctl`（包括任何 fallback）。

### 2) 分步写日志（避免输出混入总结）

- 每一个外部命令步骤产生一个独立的 `TaskLogEntry`：
  - `image-pull`（podman pull）
  - `restart-unit` / `start-unit`（systemctl）
  - `unit-health-check`（systemctl show）
  - `image-verify`（非 command meta）
  - 失败诊断（systemctl status / journalctl / podman inspect 等）
- 最终总结只负责汇总状态与短摘要，不承载命令输出。

### 3) 镜像核验（image-verify）

仅在本次任务执行了 `image-pull` 时运行。

#### 3.1 远端 digest（remote_index_digest / remote_platform_digest）

- 基础获取：对 `image ref` 做 OCI Distribution `HEAD /v2/<repo>/manifests/<tag>`，读取 `Docker-Content-Digest` 作为 `remote_index_digest`。
- 若该 digest 对应的是 index/list：
  - `GET` manifest JSON（带 Accept header 覆盖 OCI/Docker v2 的 manifest 与 list 类型）
  - 根据本机 `platform(os/arch/variant)` 选择对应 descriptor 的 digest 作为 `remote_platform_digest`
- 若是单 manifest：`remote_platform_digest = remote_index_digest`
- 远端失败处理：任何网络/鉴权/解析错误都不导致任务 failed，而是使 `image-verify` 进入 `unknown`，任务最终状态置为 `unknown`。

> 需要扩展现有 registry digest 缓存：当前 `registry_digest_cache` 仅存一个 digest，未来实现应按 `(image, platform)` 缓存 `remote_platform_digest`，并保留 `remote_index_digest` 作为诊断字段（可能需要新表或新增列）。

#### 3.2 本地 pulled_digest

在 `podman pull` 成功后，通过 `podman image inspect <image ref>` 或等价手段解析本地 image 的 platform manifest digest，记录为 `pulled_digest`。

#### 3.3 运行中 running_digest

在 unit 重启且健康检查通过后，基于 `podman ps --format json`（**必须 fresh，不复用进程级缓存**）：

- `unit -> container`：通过 `io.podman.systemd.unit` / `PODMAN_SYSTEMD_UNIT` 等 label 映射
- `container -> image id -> digest`：`podman image inspect` 解析 `RepoDigests/@sha256:` 或 `Digest` 字段

### 4) 失败诊断步骤（必执行）

当任务被判定为 `failed` 时，追加以下独立日志条目（command meta）：

- `unit-diagnose-status`：`systemctl --user status <unit> --no-pager --full`
- `unit-diagnose-journal`：`journalctl --user -u <unit> -n <N> --no-pager --output=short-precise`
- `podman-diagnose-*`（建议）：容器与镜像 inspect 的关键字段，用于确认 “容器是否换镜像/是否存在多容器/label 是否缺失”

## 前端展示（概要）

- 现有 command meta 展开逻辑可直接复用；分步日志将显著提升可读性。
- 对 `image-verify`：
  - 前端可通过白名单读取并展示 `result_message`（折叠长文本）
  - 必要时增加一个小型 digest 展示块（remote/pulled/running），避免用户必须导出 JSON 才能看见关键结果

## 兼容性与迁移

- 旧任务日志不追溯迁移；新日志格式对旧 UI 兼容（旧 UI 至少能显示 summary）。
- 如扩展 `registry_digest_cache` 存储 platform digest，需要新增 SQLite migration（实现阶段完成）。

## 测试计划（实现阶段）

- 后端：
  - 单元测试：多架构 manifest list 解析与 platform 选择；错误分支（401/timeout/无匹配 platform）
  - 集成测试：mock registry（HEAD/GET + 401 challenge）、mock podman（ps/inspect）验证 `image-verify` 三件套
- 前端：
  - UI E2E（mock）：时间线能看到分步 action，且 `image-verify` 的摘要与折叠逻辑可见

## 风险点

- 多架构 digest 口径：必须明确记录 index 与 platform digest，避免误判。
- unit->container 映射：依赖标准 label；label 缺失时需返回可解释的 reason 并在失败诊断中体现。
- 性能：远端 digest 查询需缓存与并发控制，避免大规模服务同时刷新造成外网压力。

