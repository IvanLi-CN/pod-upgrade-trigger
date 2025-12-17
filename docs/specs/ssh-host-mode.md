# SSH Host Mode（远程测试环境）与本地自更新入口：需求分析与概要设计

## 背景与目标

当前生产环境采用 **rootless Podman + Quadlet（systemd user units）** 管理容器与更新编排；但本项目在生产环境长期“不可有效工作”，而现有后端 E2E 主要依赖 `tests/mock-bin` 注入 mock 命令，难以复现真实 systemd/podman 行为与故障模式。

主人希望在开发/测试阶段配套一个“真实运行环境”用于验证后端 E2E，同时尽量不引入额外的部署与同步流程。

本工作项的目标是：

1. 引入 **SSH Host Mode**：开发机运行 `pod-upgrade-trigger` 与所有测试；当后端需要执行 host 侧动作（`podman`/`systemctl --user`/`busctl --user`/读取 Quadlet 目录/读取 auto-update 日志）时，通过 SSH 在远端 **SSH target** 上执行。
   - 当前约定：SSH target 是“测试服务器上的 Docker 测试容器”，通过 **端口映射**暴露 SSH（无需手工维护 `ssh -L/-R` 转发会话）。
2. 对上层保持透明：API/CLI 不因 SSH 模式而变化；差异仅体现在“执行后端如何与宿主交互”。
3. **自我更新永远是本地的**：自更新触发入口不与其它触发混用；由 Web UI 顶栏更新图标打开 daisyUI 对话框，完成“查看新版本/执行更新/取消”。
4. SSH 模式需覆盖除“本地自更新”外的全部功能（Manual/Webhooks/Scheduler/Tasks/Logs/Settings 等涉及 host 行为的部分）。

## 范围与非目标

### 范围（本次交付）

- 后端：
  - 内置一个“宿主执行后端（Host backend）”抽象，支持 `local` 与 `ssh` 两种实现。
  - 在 SSH 模式下：
    - `PODUP_CONTAINER_DIR`、`PODUP_AUTO_UPDATE_LOG_DIR` 解释为**远端路径**；
    - 所有原先本机执行的宿主命令与文件访问（Podman/systemd/journal/Quadlet 文件读取等）改为走 Host backend。
  - 为保证 macOS 开发机也能工作，引入“任务执行器（Task executor）”抽象，避免依赖本机 `systemd-run` 才能实现 stop/force-stop 等能力。
- Web UI：
  - 顶栏新增“更新”图标与 daisyUI modal，作为本地自更新的唯一交互入口。
  - 该入口与 GitHub webhook / 手动触发 / 调度器等入口严格解耦。
- E2E：
  - 保留现有 mock E2E（用于离线与快速回归）。
  - 新增“真实环境（SSH）E2E”执行入口与远端环境初始化/预检脚本（以可重复为第一目标）。

### 非目标（明确不做）

- 不在远端部署/运行 `pod-upgrade-trigger` 二进制（不做“远端 agent”）。
- 不做 UI 内切换目标主机/配置 profile 的复杂能力（本次 SSH target 仅通过 env/启动参数固定）。
- 不实现多测试环境并行隔离（除非在容器化隔离环境下；本次默认串行）。
- 不改变生产环境部署拓扑（本次只新增能力与测试路径）。

## 总体设计

本工作项拆成两个互相正交的维度，避免“为了 SSH 把所有逻辑揉成一团”：

1. **Host backend（宿主交互层）**：决定“命令与文件在哪执行/读取”。
   - `LocalHostBackend`：本机 `Command` + `std::fs`（现状）。
   - `SshHostBackend`：通过 OpenSSH CLI 在远端执行命令/读写必要文件。
2. **Task executor（任务调度层）**：决定“后台任务怎么跑、如何 stop/force-stop”。
   - `SystemdRunExecutor`：沿用现状（`systemd-run --user ... pod-upgrade-trigger run-task <id>`）。
   - `LocalChildExecutor`：不依赖 systemd，直接在本机 spawn 子进程 `pod-upgrade-trigger run-task <id>`，并维护 task_id -> child pid 映射用于 stop/force-stop。

推荐默认组合：

- 生产（本机 host）：`Host=local` + `Executor=systemd-run`（保持现有语义）。
- 开发/测试（macOS + 远端 SSH target）：`Host=ssh` + `Executor=local-child`（保证 stop/force-stop 可用，且无需本机 systemd）。

## 配置与约束

### SSH 模式的关键配置（必需）

- `PODUP_SSH_TARGET`：SSH 目标（`user@host` 或 `~/.ssh/config` 的 Host alias）。当前推荐使用 **Host alias** 来承载端口映射后的端口（避免把 `-p` 之类参数揉进业务配置）。
- `PODUP_CONTAINER_DIR`：远端“容器单元定义目录”（Quadlet/`.service`）；生产常见为 `~/.config/containers/systemd`，本项目 SSH target E2E 也固定为 `~/.config/containers/systemd`（由 `scripts/e2e/ssh-target/deploy.sh` 写入，以确保 quadlet 能生成对应的 `.service` 单元）。
- `PODUP_AUTO_UPDATE_LOG_DIR`：远端 auto-update 日志目录；生产常见为 `~/.local/share/podman-auto-update/logs`，本项目 SSH target E2E 默认使用一个“缺失目录”来覆盖容错路径（见 `scripts/test-e2e-ssh.sh`）。

示例：

```bash
export PODUP_SSH_TARGET="podup-test"
export PODUP_CONTAINER_DIR="/home/ivan/.config/containers/systemd"
export PODUP_AUTO_UPDATE_LOG_DIR="/home/ivan/.local/share/podup-e2e/logs-missing"
```

### 本次建议增加的可选配置（为后续扩展预留）

- `PODUP_HOST_MODE=local|ssh`：显式选择 host backend（缺省：当 `PODUP_SSH_TARGET` 存在时视为 `ssh`）。
- `PODUP_TASK_EXECUTOR=systemd-run|local-child`：显式选择任务执行器（缺省：`ssh` 模式走 `local-child`，否则优先 `systemd-run`）。
- `PODUP_SSH_CONNECT_TIMEOUT_SECS`：SSH 连接超时（缺省例如 5 秒）。
- `PODUP_SSH_ARGS`：附加 OpenSSH 参数（如 `-i`、`-p` 等；注意日志脱敏）。

### 测试环境形态（当前约定）：Docker + 端口映射

本次以“测试服务器上运行 Docker 测试容器”为基线，容器对外提供一个 SSH 入口；`pod-upgrade-trigger` 只需将 `PODUP_SSH_TARGET` 指向该入口即可。

#### 镜像目标（规范）

测试容器镜像必须满足以下“可观测行为”，而不是仅满足“安装了几个包”：

1. 通过 SSH 非交互执行命令时（`ssh target -- <cmd>`），以下命令必须可用：
   - `systemctl --user ...`（必须能连接 user systemd 与 user bus）
   - `journalctl --user ...`（用于错误诊断与验收）
   - `podman ...`（用于后端真实路径：pull/ps/auto-update/prune）
2. 镜像必须内置一个 **无副作用的 systemd user unit**（用于测试与验收，避免重启真实业务服务）：
   - 单元名固定：`podup-e2e-noop.service`
   - 行为：长期运行但不做业务动作（例如 `sleep infinity`），允许被安全地 `restart/stop/start`
3. **不要求在容器外侧手工维护 SSH 转发会话**：
   - 只允许通过 `docker run -p <host_port>:22` 端口映射暴露 SSH；
   - `PODUP_SSH_TARGET` 推荐使用 `~/.ssh/config` 的 Host alias 绑定端口。
4. 允许复用“真实业务 Quadlet 目录/auto-update 日志目录”，但容器对它们的访问默认为 **read-only**（防止测试误改生产/配置）：
   - 读取/发现：允许
   - 写入/删除：默认禁止（除非主人明确将挂载改为 rw 并在测试机上隔离）

#### Dockerfile 与启动脚本（规范）

为保证可重复性，本仓库应新增一套“可构建的测试镜像定义”，建议目录：

- `scripts/e2e/ssh-target/Dockerfile`
- `scripts/e2e/ssh-target/entrypoint.sh`

Dockerfile 的最低要求（实现方式可变，但验收必须满足“镜像目标”）：

- 基础能力：
  - `systemd` 作为 PID 1 运行（容器内需要 system instance 承载 `--user` 会话）
  - `openssh-server`（仅允许 key auth；建议关闭密码登录）
  - `dbus`（user bus / system bus 由 systemd 管理或按发行版惯例拉起）
- 容器工具链（用于真实路径）：
  - `podman`（包含 quadlet generator）
  - rootless podman 依赖（发行版不同，通常包括 `uidmap`、`slirp4netns`、`fuse-overlayfs`、`crun` 等）
- 账号模型：
  - 创建一个用于 SSH 登录与执行运维命令的用户（与主人运维账号同名/同 UID 更佳，用于读取挂载目录权限）
  - 确保该用户通过 SSH 登录后能直接执行 `systemctl --user` 与 `podman`（必要时设置 subuid/subgid）
- 测试专用 unit：
  - 镜像内必须提供 `podup-e2e-noop.service`（systemd user unit），并确保可被启动与重启（用于 SSH E2E 验收）。
- 启动脚本职责（entrypoint）：
  - 生成/加载 host keys
  - 启动 `sshd`
  - 确保 systemd 必要 unit 已就绪（按发行版实现）

> 备注：systemd-in-docker 与 rootless podman-in-docker 的实现细节会随发行版变化；本 spec 约束的是“可观测行为 + 最低组件”，实现可在落地时根据实际测试环境迭代收敛。

- 测试服务器侧（示例）：
  - `docker run -d --name podup-test -p 2222:22 ... <image>`
  - 如需复用真实业务 Quadlet 目录与日志目录，可在启动容器时通过 bind mount 挂载到容器内，并将 `PODUP_CONTAINER_DIR` / `PODUP_AUTO_UPDATE_LOG_DIR` 指向容器内路径。
- 开发机侧建议在 `~/.ssh/config` 配置 Host alias（示例）：

```sshconfig
Host podup-test
  HostName <test-server-hostname-or-ip>
  User <ops-user>
  Port 2222
```

说明：

- 本设计不要求、也不依赖在测试服务器上手工维护 `ssh -L/-R` 的端口转发会话。
- 本项目不实现也不依赖 `docker exec`/Docker API；“容器如何启动、如何映射端口、挂载哪些目录”属于测试环境初始化脚本的职责。

#### docker run（推荐模板）

下面给出一份“可讨论的推荐模板”，用于让 SSH target 能跑 systemd + podman（后续实现时以实际测试结果为准）：

```bash
docker run -d --name podup-test \
  --privileged \
  --cgroupns=host \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
  -p 2222:22 \
  -v /home/<ops-user>/.ssh/authorized_keys:/etc/ssh/authorized_keys/<ops-user>:ro \
  -v /home/<ops-user>/.config/containers/systemd:/mnt/quadlet-real:ro \
  <image>
```

挂载路径约定：

- 业务 Quadlet 目录挂载到 `/mnt/quadlet-real`（只读）
- auto-update 日志目录（可选）挂载到 `/mnt/podman-auto-update-logs`（只读）
  - 若宿主目录不存在，**不要**在 `docker run` 中添加该 bind mount（避免 Docker 以 root 身份创建宿主目录，导致权限与语义混乱）。
  - 在 SSH Host Mode 中，`PODUP_AUTO_UPDATE_LOG_DIR` 缺失/不可读会被视为“无日志可读”，不会触发目录创建。
- `PODUP_CONTAINER_DIR` 在 SSH 模式下应指向容器内路径（例如上述 `/mnt/quadlet-real`）
- `PODUP_AUTO_UPDATE_LOG_DIR`（如需读取）在 SSH 模式下应指向容器内路径（例如 `/mnt/podman-auto-update-logs`）

### 远端前置条件（主人保证）

- 远端（即 SSH target；当前为 Docker 测试容器）以“具备权限的同一运维账号”登录，能完成日常运维：
  - `podman` rootless 可用；
  - `systemctl --user` 可用（需要 user systemd 实例与 user bus；在非容器化主机上通常需要 linger：`loginctl enable-linger <user>`）；
  - `busctl --user` / `journalctl --user` 可用（若缺失可降级，但会影响诊断能力）。

## SSH Host backend 设计

### 1) 远端命令执行

统一入口：`HostBackend::exec(program, argv, stdin, timeout)`，上层不再直接 `Command::new("podman")`。

SSH 实现策略：

- 使用 OpenSSH CLI（`ssh`）作为唯一依赖；不在本次实现 SSH 协议。
- 默认启用非交互模式：`BatchMode=yes`（避免卡住测试与服务进程）。
- Host key 策略：默认采用 `StrictHostKeyChecking=accept-new`（符合本次约定）；后续可通过 `PODUP_SSH_ARGS` 覆盖为更严格策略。
- **命令白名单**：只允许执行本项目需要的固定命令集（例如 `podman`、`systemctl`、`busctl`、`journalctl`、`cat`、`ls`、`stat`、`rm`、`mkdir`、`tee`、`sh`），其余拒绝并记录审计日志。
- **参数安全**：
  - 远端路径与 unit 名称均做严格校验（例如：路径必须绝对路径且只含 `[A-Za-z0-9._/:-]` 等安全字符；unit 必须匹配 systemd unit 命名规则的子集）。
  - 所有对外输入（HTTP body/query/env）在进入远端执行前完成校验与规范化，避免注入。
- 结构化日志：
  - `task_logs.meta` 中记录 `runner=ssh`、`target`（脱敏）、`command`、`argv`、`exit/stdout/stderr`（截断）以便排障。

### 2) 远端文件访问（PODUP_CONTAINER_DIR / AUTO_UPDATE_LOG_DIR）

SSH 模式下，这两个目录属于远端，Host backend 需要提供最少集的文件操作：

- `read_file(path)`：读取 Quadlet/Service 文件内容（用于 service auto-discovery 与解析）。
- `list_dir(path)`：列出 `.container`/`.service`/`.network`/`.volume` 等文件（用于发现）。
- `exists(path)`：启动时预检与可读性检查。
- `create_dir_all(path)`：仅允许对“本项目拥有的目录”执行创建（例如测试专用目录）。**不得**为 `PODUP_AUTO_UPDATE_LOG_DIR` 自动创建目录；该目录应由 `podman auto-update` 自身产生，缺失时视为“无日志可读”并跳过解析。
- `tail_file(path, n)`（可选）：读取远端 auto-update 日志尾部（用于 UI 展示/诊断）。

实现上可先采用 `ssh target -- <posix cmd>` 的方式（如 `ls -1`、`cat`、`tail`），并配合“路径字符集约束”保证安全；后续如需更强健可切换到 `sftp -b` 的 batch 模式实现读写（仍是 OpenSSH 生态，避免额外依赖）。

### 3) 预检（启动时与 E2E 前）

在 HTTP server 启动（或第一次执行 host 动作）时执行一次 preflight：

- `ssh` 连通性：`ssh <target> true`（使用与实际执行一致的 SSH 选项，如 `BatchMode`、`StrictHostKeyChecking`）
- 关键命令可用性：`podman --version`、`systemctl --user --version`、（可选）`busctl --user --version`、`journalctl --version`
- `systemctl --user` 可用性检查：
  - 若无法连接 user bus / 用户实例，返回明确错误并提示修复建议（主机场景优先提示 `loginctl enable-linger <user>`；容器场景提示检查 systemd/user bus 是否按预期启动）。
- 远端目录存在性：
  - `PODUP_CONTAINER_DIR`：必须可读（必要时可创建测试专用目录）。
  - `PODUP_AUTO_UPDATE_LOG_DIR`：仅做可读性检查，**不得**自动创建；缺失/不可读时跳过 auto-update 日志解析与告警提取。

## Task executor 设计（保证 SSH 模式覆盖全部功能）

### 问题陈述

当前实现对部分任务（例如 GitHub webhook）依赖 `systemd-run --user --unit=webhook-task-...` 来提供：

- 后台执行 `run-task`
- 通过 `systemctl --user stop/kill` 实现 stop/force-stop

但在“开发机为 macOS + host 为 SSH 远端”的模式下，本机不存在 systemd，继续依赖 `systemd-run/systemctl --user` 将导致 stop/force-stop 等功能不可用，从而不满足“SSH 模式覆盖全部功能”的要求。

### 方案：LocalChildExecutor

新增一个本机任务执行器：

- Dispatch：
  - `spawn(current_exe, ["--run-task", task_id], env=collect_run_task_env())`
  - 将 `task_id -> child_pid` 维护在进程内表（内存映射），并将 pid **持久化到本机 pidfile**，以支持 `http-server` “每个请求派生一个 `server` 进程”的模型下跨进程 stop/force-stop：
    - pidfile 路径：`$PODUP_STATE_DIR/task-pids/<task_id>.pid`（若未设置 `PODUP_STATE_DIR`，回退到系统临时目录 `$(tmp)/pod-upgrade-trigger/task-pids/<task_id>.pid`）。
    - `run-task` 子进程退出时会主动清理自身 pidfile，避免遗留脏映射；同时 stop/force-stop 遇到 `ESRCH` 也会清理 pidfile 作为兜底。
- Stop/Force-stop：
  - `stop`: 对映射中的 `pid` 发送 `SIGTERM`（不依赖外部 `kill` 命令）。
  - `force-stop`: 对映射中的 `pid` 发送 `SIGKILL`。
  - 若找不到 `pid`（子进程已退出 / 进程重启导致映射丢失），API 返回明确错误并写入 `task_logs`（不 hang）。

语义差异（需要在 UI/文档说明）：

- `http-server` 模式下，每个请求都会派生新的 `server` 进程：因此 stop/force-stop **不能**依赖单进程内存映射，必须通过 pidfile 跨进程定位 run-task 子进程。
- 本机进程重启/崩溃后：
  - 若 run-task 子进程仍在运行，pidfile 仍可用于 stop/force-stop；
  - 若 pidfile 遗留但 pid 已不存在，API 会返回明确错误并清理 pidfile（避免下一次误判）。
- 生产环境仍可使用 systemd-run，保持可控性与持久性。

## 本地自我更新（UI 入口）设计

### 约束

- 自我更新 **永远只作用于本机运行的二进制**（即运行 HTTP server 的那台机器）。
- 触发入口必须独立，不与 webhook/manual/scheduler 入口共用。
- SSH 模式下，自更新仍然只更新本机，不尝试更新远端 SSH target（测试容器/测试机）。

### UI 交互

- 顶栏标题右侧固定显示“更新”图标（与“新版本提示 badge”并存或合并）。
- 点击图标弹出 daisyUI modal：
  - 展示：当前版本、最新版本（若可得）、上次检查时间。
  - 操作：
    - “查看新版本”：触发版本检查（复用 `GET /api/version/check` 的结果）。
    - “更新”：触发 `POST /api/self-update/run`（可支持 dry-run）。
    - “取消”：关闭对话框。

### 后端 API（建议）

- `POST /api/self-update/run`：立即执行一次本地 `PODUP_SELF_UPDATE_COMMAND`（或未来内建 updater），并把结果写入任务/日志体系。
- 可选 `GET /api/self-update/status`：返回是否正在运行、最近一次结果摘要等（用于 UI 细节）。

注：版本检查的更多细节可复用现有规格 `docs/specs/version-check-on-focus.md`，本工作项只新增“UI 触发更新”与“入口隔离”的约束。

## E2E：真实环境（SSH）测试方案

### 总体策略

- 保留 `scripts/test-e2e.sh` + `tests/mock-bin`：用于离线/快速回归。
- 新增可选入口运行真实环境用例（不默认在 CI 跑）：
  - 通过环境变量提供 `PODUP_SSH_TARGET` 与远端路径（当前路径为“复用真实业务 Quadlet 目录/日志目录”）；
  - 由脚本负责启动/销毁 Docker 测试容器、端口映射与目录挂载，并验证 `podup-e2e-noop.service` 可用；
  - 测试断言以“真实 systemctl/podman 行为”为准，而非 mock 日志。

### 建议新增脚本（命名仅建议）

- `scripts/test-e2e-ssh.sh`：
  - 从开发机执行，输入参数为 `root@<host>`；
  - 先跑基线验收 `scripts/e2e/test-host/verify.sh root@<host>`（确保任务 1 环境未被破坏）；
  - 通过 `scripts/e2e/ssh-target/deploy.sh root@<host>` + `scripts/e2e/ssh-target/verify.sh root@<host>` 确保 SSH target 可用；
  - 导出并固定本次 E2E 所需 env：
    - `PODUP_E2E_SSH=1`（用于让 `tests/e2e_ssh.rs` 默认跳过，避免 CI 误跑）
    - `PODUP_SSH_TARGET=ssh://ivan@<host>:2222`
    - `PODUP_CONTAINER_DIR=/home/ivan/.config/containers/systemd`
    - `PODUP_AUTO_UPDATE_LOG_DIR=/home/ivan/.local/share/podup-e2e/logs-missing`
  - 运行 `cargo test --locked --test e2e_ssh -- --nocapture`（不得注入 `tests/mock-bin` 到 `PATH`）。

### 并发与隔离

- 默认：同一 SSH target 只跑一个 E2E 进程，避免互相踩踏（串行）。
- 如需并行：优先通过“多个 Docker 容器 + 不同端口映射”的方式提供多个 SSH target；或在更强隔离环境（LXC/systemd-nspawn/独立 VM）下通过不同 target/不同目录隔离实现。

## 兼容性与迁移

- 不设置 `PODUP_SSH_TARGET` 时行为保持不变（local host backend）。
- 现有部署文档与运行方式不需要立即修改；SSH 模式为新增能力。
- UI 自更新入口为新增功能，但不改变现有自更新 scheduler 的语义（仍可通过 cron 自动运行）。

## 风险点与待确认问题

1. **远端 `systemctl --user` 非交互可用性**：需要确认 SSH target 内 user systemd 实例与 user bus 可用；主机场景可能需要 linger，容器场景需要按约定启动 systemd/user bus。
2. **远端路径约束是否足够**：为安全会限制路径字符集；主人需确认远端目录命名无空格等特殊字符。
3. **Stop/force-stop 语义**：LocalChildExecutor 在进程重启后无法控制旧任务，需要 UI/接口给出清晰反馈。
4. **真实环境 E2E 的稳定性**：依赖远端环境状态（镜像缓存、网络、registry、podman 版本）；需要在脚本中尽量做显式 preflight 与清理。

## 测试计划（建议）

- 单元测试：
  - SSH 模式参数校验（target/path/unit）。
  - Host backend 命令白名单与 meta 记录（截断规则）。
  - LocalChildExecutor stop/force-stop 行为。
- 集成/E2E：
  - 在远端准备最小 Quadlet 单元，验证 `/api/manual/services` discovery 与 `POST /api/manual/deploy`（pull + restart；auto-update excluded）的主路径行为；可选覆盖 legacy `POST /api/manual/trigger`（restart-only）。
  - 选取 1-2 条最关键生产路径（GitHub webhook dispatch + unit restart + 失败诊断抓取）做真实环境回归。

## 交付物（本次必须完成）

### 后端（功能）

- Host backend 抽象落地：
  - `local`：沿用现有本机 `Command`/`fs` 语义
  - `ssh`：通过 OpenSSH CLI 在 `PODUP_SSH_TARGET` 上执行，并对外保持 API/CLI 行为不变
- SSH 模式覆盖范围：
  - 所有涉及宿主机命令与宿主机文件访问的路径必须走 Host backend（Podman/systemd/journal/读 Quadlet/读 auto-update 日志等）
  - `PODUP_CONTAINER_DIR` / `PODUP_AUTO_UPDATE_LOG_DIR` 在 SSH 模式下解释为 **SSH target 内路径**
- Task executor 抽象落地：
  - SSH 模式默认 `local-child`（保证 macOS 开发机无 systemd 仍能 dispatch/stop/force-stop）
  - 非 SSH 模式优先 `systemd-run`（保持生产语义）
- 安全与可观测性：
  - SSH 模式命令白名单 + 参数校验（unit/path 等）
  - 任务日志/事件中必须能区分本次操作是 `host_backend=ssh` 还是 `local`

### 测试环境（Docker）

- 仓库内提供可构建镜像定义：
  - `scripts/e2e/ssh-target/Dockerfile`
  - `scripts/e2e/ssh-target/entrypoint.sh`
- 镜像必须内置并可运行 `podup-e2e-noop.service`（systemd user unit，用于验收与 E2E）

### 测试（E2E）

- 新增 SSH E2E 入口脚本（建议）：
  - `scripts/test-e2e-ssh.sh`
  - 该脚本不得注入 `tests/mock-bin` 到 `PATH`
  - 脚本应包含：启动/销毁测试容器、preflight、运行测试用例
- 新增或分组测试用例（建议 `tests/e2e_ssh.rs`，或在现有 `tests/e2e.rs` 里以 feature/ignore 分组）：
  - 至少覆盖：远端目录 discovery、远端 systemctl --user restart、远端命令 meta 记录、错误分支（SSH 不可达/权限不足）

## 验收标准（必须满足）

### A. 测试镜像与 SSH 连通性

建议以脚本化方式“一键复现与验收”（端口固定 `2222:22`，容器名固定 `podup-test`）：

- 基线（任务 1 不得被破坏）：
  - `./scripts/e2e/test-host/verify.sh root@<host>` 仍 PASS
- 部署 SSH target（从开发机执行，在测试机上构建/组装镜像并启动容器）：
  - `./scripts/e2e/ssh-target/deploy.sh root@<host>`
  - 如需强制重建镜像：`PODUP_SSH_TARGET_REBUILD=1 ./scripts/e2e/ssh-target/deploy.sh root@<host>`
- 自动化验收（从开发机执行）：
  - `./scripts/e2e/ssh-target/verify.sh root@<host>`

1. 能在测试服务器上构建并运行测试容器（端口映射模式）：
   - 通过 `scripts/e2e/ssh-target/deploy.sh` 在远端构建/组装镜像并启动容器（端口固定 `2222:22`）
2. 开发机通过 Host alias（`~/.ssh/config`）可直接 SSH 到容器，并默认 `StrictHostKeyChecking=accept-new` 可工作：
   - `ssh podup-test -- true` 返回 0（alias 指向 `Port 2222`）
   - 或直接：`ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -p 2222 ivan@<host> -- true` 返回 0
3. 在容器内（通过非交互 SSH 执行）以下命令返回 0：
   - `systemctl --user list-units --no-pager`
   - `journalctl --user -n 1 --no-pager`
   - `podman --version`
4. `podup-e2e-noop.service` 可被安全重启（通过 SSH 执行）：
   - `systemctl --user restart podup-e2e-noop.service` 返回 0

### B. SSH Host Mode 行为

1. 本机启动后端，配置 SSH 模式（至少设置 `PODUP_SSH_TARGET`、`PODUP_CONTAINER_DIR`、`PODUP_AUTO_UPDATE_LOG_DIR`）后：
   - `GET /api/settings` 正常返回
   - `GET /api/manual/services` 正常返回，并包含：
     - `podup-e2e-noop.service`（manual source）
     - `discovered.units` 非空（来自 `PODUP_CONTAINER_DIR` 的远端扫描）
   - 若 `PODUP_AUTO_UPDATE_LOG_DIR` 在 SSH target 内不存在/不可读：
     - 后端不得创建该目录
     - 后端不得因该目录缺失而导致请求失败或 hang（最多记录 debug/warn 并跳过日志解析）
2. 触发一次“重启单元”后端任务（以 `podup-e2e-noop.service` 为准）：
   - 任务执行成功（HTTP 200/202，或与现有语义一致）
   - 任务日志中能看到 `host_backend=ssh` 且包含远端执行的 `systemctl --user restart ...` 的结果摘要（stdout/stderr 截断）
3. SSH 不可达/认证失败时：
   - API 返回明确错误（不 hang）
   - 事件/日志中记录失败原因（不泄露敏感信息）

### C. SSH E2E 测试

1. 新增的 SSH E2E 入口可以一键跑通：
   - `./scripts/test-e2e-ssh.sh` 在本机运行通过（前提：测试服务器可达、容器可启动）
2. 现有 mock E2E 不受影响：
   - `./scripts/test-e2e.sh` 仍可通过（保持离线回归能力）
