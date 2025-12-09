# 本机 systemd + GitHub Release 部署方案

本文在现有 `container-deploy.md` 的基础上，给出“放弃容器常驻运行、改为本机 systemd（user 服务）+ GitHub Release 二进制发布”的需求与里程碑规划。目标是先在当前这台机器上落地一套稳定的生产形态，不考虑数据迁移。

## 一、背景与范围

- 当前生产形态：
  - 使用 Docker 容器直接运行 `pod-upgrade-trigger`。
  - HTTP 入口由容器内进程提供，对外由 Traefik / webhook-proxy 反向代理。
- 运维现有思路：
  - 将服务迁移到宿主 user systemd，镜像仅作为“发行载体”（通过 Podman 从镜像中复制二进制）。
- 本文调整后的目标：
  - 运行形态仍改为 **宿主 user systemd**。
  - **不再依赖容器镜像作为发行载体**，改用 **GitHub Release 附件提供二进制**。
  - 保持对外入口（Traefik / webhook-proxy）路径与行为不变。
- 不在本次范围：
  - 数据迁移（数据库 / 状态目录布局）不在本次需求内。
  - 是否继续构建容器镜像仅作为 CI/测试产物，不纳入本轮部署决策。

## 二、目标运行形态（Target State）

### 2.1 服务进程形态

- 在宿主机上以 **user systemd** 运行：
  - Unit 名称（建议）：`pod-upgrade-trigger-http.service`。
  - 运行用户：`ivan`（或等价非 root 用户）。
  - 启动命令：
    - `ExecStart=/home/<user>/.local/bin/pod-upgrade-trigger http-server`
  - 监听地址：
    - 默认 `0.0.0.0:25111`（或不设置 `PODUP_HTTP_ADDR`，使用程序默认值），便于容器内的 webhook-proxy 通过 `host.containers.internal:25111` 访问宿主。

### 2.2 上游与网络

- Traefik / webhook-proxy：
  - 维持现有链路：Traefik（容器） → `webhook-proxy:9700` → `host.containers.internal:25111`。
  - 不修改任何 upstream 配置，对外 API/回调 URL 不变；只是将 25111 上的提供者从容器进程切换为宿主 user systemd 进程。
  - 若未来考虑移除 webhook-proxy、让 Traefik 直连宿主，需要单独设计与评估（不在本方案内）。
- 不再长期运行 `pod-upgrade-trigger` 容器：
  - 容器镜像可以继续存在，用于测试或其它用途，但**不再作为生产服务的长期进程**。

### 2.3 端口占用与切换顺序

- 容器版与宿主版不能同时监听 `25111`。
- 正确的并行验证与切换步骤：
  1. 先用临时端口（例如 `25211`）执行 `pod-upgrade-trigger http-server --http-addr 0.0.0.0:25211` 做功能验证，容器版继续占用 `25111`。
  2. 验证通过后停止容器版（如 `systemctl --user stop pod-upgrade-trigger.service` 或对应 Podman unit）。
  3. 启用并启动宿主版：`systemctl --user enable --now pod-upgrade-trigger-http.service`，占用 `0.0.0.0:25111`。
  4. 全程不改 Traefik / webhook-proxy 配置，它们仍探测宿主 `25111`。

### 2.4 宿主环境前提

- 宿主机需要满足：
  - 支持 user systemd（`systemctl --user` 可用，必要时启用 linger）。
  - 用户 `ivan` 的 `$HOME` 路径稳定（例如 `/home/<user>`），并且：
    - 存在 `~/.local/bin`，在用户 PATH 中；
    - `pod-upgrade-trigger` 二进制放置于该目录。
  - 可用工具：
    - `curl` 或 `wget`（用于下载 Release 附件）；
    - 如需更友好的 JSON 解析，可选 `jq`，但脚本需在无 `jq` 情况下也能工作。
  - 网络：
    - 能够访问 GitHub Release 下载地址（如受限环境，需提前规划代理 / 缓存）。

## 三、发布与更新需求

### 3.1 GitHub Release 作为唯一发布载体

- CI 要求：
  - 每次 Release 时，为 Linux 生产环境构建单一二进制：
    - 当前 M2 实现的目标平台：`x86_64-unknown-linux-gnu`（Linux amd64，glibc）。
  - 将构建产物作为 Release 附件上传：
    - 当前 M2 实现的附件列表固定为：
      - `pod-upgrade-trigger-x86_64-unknown-linux-gnu`（可执行二进制）
      - `pod-upgrade-trigger-x86_64-unknown-linux-gnu.sha256`（对应的 SHA256 校验文件）
  - Release 标签与版本号：
    - 使用 SemVer（如 `v1.2.3`）。
    - 可视需求维护一个 `latest` tag 或通过 GitHub API 获取“最新稳定版本”。

### 3.2 更新链路（唯一逻辑链）

更新链路设计为**一条唯一的流程**，由不同触发方调用，不区分“手动版”和“自动版”的实现。

链路步骤：

1. **确定目标版本**：
   - 通过 GitHub API 查询最新 Release（或指定 channel，如 stable / beta）。
   - 或从本地配置中读取“目标版本”。
2. **从 Release 下载产物**：
   - 使用 `curl` / `wget` 下载对应附件到临时路径：
     - 如：`~/.local/bin/pod-upgrade-trigger.new`。
   - 使用 Release 附带的 `.sha256` 校验文件，在下载目录中完成校验。
3. **原子替换现有二进制**：
   - 确保临时文件 `chmod +x`。
   - 使用 `mv` 将 `*.new` 替换到最终路径：
     - `/home/<user>/.local/bin/pod-upgrade-trigger`
   - 可选：保留 `pod-upgrade-trigger.old` 以便快速回滚。
4. **重启 HTTP 服务**：
   - `systemctl --user restart pod-upgrade-trigger-http.service`
   - 失败时：
     - 同步返回错误码；
     - 日志留在 `journalctl --user` 中供排查。

**手动更新 vs 自动更新**：

- 实现上只维护一份脚本（例如 `scripts/update-pod-upgrade-trigger-from-release.sh`）：
  - 手动更新：管理员直接运行脚本。
  - 自动更新：user systemd 的 oneshot service / timer 调用同一脚本。
- 不再有“两条不同的下载路径”，避免维护成本和行为差异。

### 3.3 可靠性与安全要求

- 失败保护：
  - 下载失败 / 校验失败时，**不得覆盖现有二进制**。
  - 重启失败时，保持现有进程或快速回滚，不出现“服务消失”状态。
- 版本可观测性：
  - `pod-upgrade-trigger --version` 输出需包含版本号与 commit 信息，便于对齐 Release。
  - 更新脚本在日志中记录：
    - 当前版本 → 目标版本；
    - 下载来源（Release tag / URL）；
    - 成功或失败原因。
- 可选：与现有 auto-update 体系集成：
  - 如需要在现有 `/tasks` / `/events` JSONL 里记录自更新事件，可在脚本内追加 JSONL 输出，或者由上游调用方（如 `podman-update-manager.ts`）在检测到新版本时调用脚本并记录结果。

## 四、迁移约束与兼容性

- 不动的数据 / 状态：
  - 不在本次计划中对数据库或状态目录（`PODUP_STATE_DIR`）做迁移方案；沿用当前路径或另行设计，单独文档说明。
- 架构和 ABI 兼容：
  - 要求 Release 附件与生产机 CPU 架构、glibc 版本兼容。
  - 建议优先使用 `musl` 静态链接二进制，以降低 ABI 兼容风险。
- 网络约束：
  - 若生产机无法直接访问 GitHub，需要事先确认：
    - 是否通过代理下载；
    - 是否需要在内部仓库镜像 Release 附件。

## 五、里程碑规划

### M0：方案确认与文档落地（本文件）

- 明确：
  - 从 Docker 容器部署迁移到宿主 user systemd 的目标。
  - GitHub Release 作为主要发行载体的设计。
  - 更新链路：单一脚本，被手动与定时两种方式共用。
- 输出：
  - 本文档合并到仓库，作为后续实现的设计依据。

### M1：基础运行形态落地（手动部署）

- 内容：
  - 在仓库中添加 `pod-upgrade-trigger-http.service` 示例（`systemd/` 或 `docs/` 示例）。
    - 示例 user unit：`systemd/pod-upgrade-trigger-http.user.service.example`，配套 env 示例：`systemd/pod-upgrade-trigger-http.env.example`。
    - env 示例直接拷贝现网容器 env 的字段，仅在可见地址（保持 `0.0.0.0:25111` 以兼容 webhook-proxy）和自更新变量上做增补。
  - 在当前机器上手动：
    - 将 Release 二进制放到 `/home/<user>/.local/bin/pod-upgrade-trigger`。
    - 如需先行验证，先用 `pod-upgrade-trigger http-server --http-addr 0.0.0.0:25211`（临时端口）跑一遍；验证通过后停止容器版 unit，再启用 `systemctl --user enable --now pod-upgrade-trigger-http.service` 占用 `25111`。
  - Traefik / webhook-proxy 配置保持不变（Traefik → webhook-proxy:9700 → host.containers.internal:25111），仅替换 25111 上的进程提供者。
- 验证：
  - 功能回归：所有现有 webhook / API 路径正常工作。
  - 日志与监控：确认 journald 中能看到服务日志。

### M2：CI 产物与 Release 规范化

- 内容：
  - 在 CI 中增加面向 Release 事件的 Linux 生产环境二进制构建 job（`release-binaries`）。
  - 目标平台固定为 `x86_64-unknown-linux-gnu`，仅构建单一可执行文件。
  - 将构建产物作为附件上传至对应 GitHub Release，附件名称固定为：
    - `pod-upgrade-trigger-x86_64-unknown-linux-gnu`
    - `pod-upgrade-trigger-x86_64-unknown-linux-gnu.sha256`
  - 更新 `README` / docs，说明 Release 附件用法。
- 验证：
  - 能够在任意新环境中，仅通过如下步骤完成部署：
    - 从目标 Release 下载上述两个附件；
    - 在下载目录中执行 `sha256sum -c pod-upgrade-trigger-x86_64-unknown-linux-gnu.sha256` 校验通过；
    - 将二进制移动到 `~/.local/bin/pod-upgrade-trigger`，执行 `chmod +x ~/.local/bin/pod-upgrade-trigger`；
    - 按 M1 配置并启用 `systemctl --user enable --now pod-upgrade-trigger-http.service`。

### M3：更新脚本实现与手动更新流程

- 内容：
  - 在仓库中添加 `scripts/update-pod-upgrade-trigger-from-release.sh`：
    - 调用 GitHub API 或固定 URL 查询最新 Release；
    - 下载产物到临时文件，校验并原子替换；
    - 重启 `pod-upgrade-trigger-http.service`；
    - 记录日志（包含版本信息和错误信息）。
  - 在当前机器上使用该脚本完成一次“手动更新”演练。
- 验证：
  - 在不破坏现有数据与配置的前提下，完成一次从版本 A → 版本 B 的更新。
  - 出错路径测试：模拟下载失败 / 校验失败，确保不会破坏现有服务。

### M4：自动更新（systemd timer）

- 内容：
  - 示例 user unit：`systemd/pod-upgrade-trigger-updater.user.service.example`（`Type=oneshot`，`ExecStart` 调用更新脚本）。
  - 示例 timer：`systemd/pod-upgrade-trigger-updater.user.timer.example`（`OnBootSec=5m`、`OnUnitActiveSec=6h`，`Persistent=true`）。
  - 启用步骤（user systemd）：
    1. 将上述两个示例复制到 `~/.config/systemd/user/`。
    2. 按实际部署路径修改 `ExecStart`，必要时用 `Environment=TARGET_BIN=...`、`Environment=PODUP_RELEASE_BASE_URL=...` 或 `EnvironmentFile=%h/.config/pod-upgrade-trigger-updater.env` 覆盖目标二进制和下载源。
    3. `systemctl --user daemon-reload`
    4. `systemctl --user enable --now pod-upgrade-trigger-updater.timer`
    5. 使用 `systemctl --user status pod-upgrade-trigger-updater.timer`、`journalctl --user -u pod-upgrade-trigger-updater.service` 检查运行。
  - 确保：
    - 定时任务在 journald 中有清晰日志；
    - Release 不可用或下载失败时返回非 0，不影响现有 `pod-upgrade-trigger-http.service` 正常运行。
- 验证：
  - 通过手工制造新 Release 或模拟版本号差异，验证定时任务能够自动更新并重启服务。

### M5：自更新报告 + `/tasks` 导入

- 自更新执行器脚本：`scripts/self-update-runner.sh`。它调用 `scripts/update-pod-upgrade-trigger-from-release.sh`，不会访问 SQLite；每次运行结束都会生成一份 JSON 报告（即使失败）。支持 dry-run：通过 `--dry-run` 或 `PODUP_SELF_UPDATE_DRY_RUN=1|true|yes|on` 只检查下载与校验，不替换二进制也不重启服务，仍会输出报告。
- 报告目录：
  - `PODUP_SELF_UPDATE_REPORT_DIR` 设置且非空时优先使用；
  - 否则落在 `${PODUP_STATE_DIR:-/srv/app/data}/self-update-reports`；
  - 文件名 `self-update-<timestamp>-<pid>.json`（先写 `.json.tmp` 再 `mv` 原子落盘）。
- 报告字段（最小集）：`type="self-update-run"`、`dry_run`（布尔，缺省视为 false）、`started_at`、`finished_at`、`status`、`exit_code`、`binary_path`、`release_tag`、`stderr_tail`、`runner_host`、`runner_pid`，时间为 Unix 秒。
- 内建调度（主程序线程）：
  - 通过环境变量启用：`PODUP_SELF_UPDATE_COMMAND`（必填，通常指向 `scripts/self-update-runner.sh`）、`PODUP_SELF_UPDATE_CRON`（必填，支持 `*/N * * * *` 或 `0 */N * * *` 两种子集语法）、`PODUP_SELF_UPDATE_DRY_RUN`（可选，1/true/yes/on 表示 dry-run）。
  - 配置有效时，`pod-upgrade-trigger http-server` 会在后台线程按 cron 周期调用自更新执行器；上一轮未结束时会跳过本轮，避免重叠。
  - 配置缺失或表达式不符合子集语法时，仅记录 warning 日志并禁用内建调度，可继续使用外部 crontab。
  - 执行器生成的报告仍由导入线程每 60 秒扫描并写入 `/tasks`，可在 UI 看到 kind=self-update / type=self-update-run 的任务记录。
- crontab 示例（建议用执行器而不是直接跑更新脚本）：

  ```
  */30 * * * * PODUP_STATE_DIR=/srv/podup /opt/pod-upgrade-trigger/scripts/self-update-runner.sh >>$HOME/.local/share/podup-self-update.log 2>&1
  - 需要预检时可加 `--dry-run`，只验证 Release 可用性与校验值。
  ```

- 导入逻辑：
  - `pod-upgrade-trigger http-server` 每分钟扫描报告目录并导入新的 `.json`；
  - 成功导入后重命名为 `.json.imported`，避免重复处理；
- 导入的任务出现在 `/tasks` / UI，`type=self-update-run`，单位固定 `pod-upgrade-trigger-http.service`，日志 action=`self-update-run`，任务 meta 中包含 `dry_run` 标记（旧报告未带字段时默认为 false）。

## 六、后续工作

- 在 M1–M4 实现过程中，需要补充：
  - systemd unit 示例文件；
  - 更新脚本实现；
  - CI 配置变更与 Release 规范说明。
- 每个里程碑完成后，应在本仓库的变更记录 / PR 描述中简要回顾对应目标与验证结果，以便后续运维和开发追踪。
