# 生产部署卡片：宿主 systemd 版

> 设计与里程碑背景见 `docs/host-systemd-release-deploy.md`；本卡片聚焦“现网容器版 → 宿主 user systemd 版”的生产运行与切换步骤。

## 1. 概览

- 现状：Traefik（容器） → `webhook-proxy:9700` → `host.containers.internal:25111` → 容器版 `pod-upgrade-trigger`（Quadlet unit）。
- 目标：保持入口链路不变，只将 25111 上的提供者切换为宿主 user systemd 进程（`pod-upgrade-trigger-http.service`）。
- 核心要求：复用现有 SQLite DB 与状态目录，避免悄悄创建新 DB；切换按“临时端口验证 → 停容器 → 起宿主”顺序执行，并提供可回滚路径。
- Web UI：Release 二进制已内嵌 `web/dist`，宿主部署无需单独同步前端；若在 `${PODUP_STATE_DIR}/web/dist` 提供自定义 bundle 会优先于内嵌版本，删除后自动回退。

## 2. 现网架构（As-Is）

- 拓扑：Traefik（容器） → `webhook-proxy:9700` → `host.containers.internal:25111` → 容器内 `pod-upgrade-trigger http-server`。
- 关键路径与文件：
  - 容器 env：`/srv/pod-upgrade-trigger/pod-upgrade-trigger.env`
  - 状态目录（宿主）：`/srv/pod-upgrade-trigger/data/`（容器内挂载为 `/srv/app/data`）
  - SQLite DB：`/srv/pod-upgrade-trigger/data/pod-upgrade-trigger.db`
- 运行单元：Quadlet / Podman unit（示例名 `pod-upgrade-trigger.service`，以实际名称为准）。

## 3. 目标架构（To-Be）

- 拓扑保持：Traefik（容器） → `webhook-proxy:9700` → `host.containers.internal:25111`。
- 服务提供者：宿主 user systemd 进程 `pod-upgrade-trigger-http.service`（运行用户 `deploy`）。
  - `ExecStart=/home/<user>/.local/bin/pod-upgrade-trigger http-server`
  - `PODUP_HTTP_ADDR=0.0.0.0:25111`（或使用默认值，同样监听 0.0.0.0:25111）
- 数据与状态（推荐方案）：**复用现有 DB**
  - `PODUP_STATE_DIR=/srv/app/data`
  - `PODUP_DB_URL=sqlite:///srv/app/data/pod-upgrade-trigger.db`
  - 切换后宿主进程接管同一 DB，历史任务/事件可直接查看。

## 4. DB 复用与迁移策略

- 推荐方案（默认执行）：
  - 保持 `PODUP_STATE_DIR=/srv/app/data` 与 `PODUP_DB_URL=sqlite:///srv/app/data/pod-upgrade-trigger.db` 不变。
  - 停容器后，由宿主版独占写入同一 DB；严禁在生产上默默创建新 DB 文件作为迁移结果。
- 可选冷迁移（仅在未来确需变更路径时执行，需维护窗口）：
  1. 停容器版 unit 与宿主版 unit，确保无进程占用 DB。
  2. 将旧 DB 拷贝到新目录（例如 `/srv/pod-upgrade-trigger`）：`cp -a /srv/pod-upgrade-trigger/data/pod-upgrade-trigger.db <新目录>/`。
  3. 更新 env 中的 `PODUP_STATE_DIR` 与 `PODUP_DB_URL` 指向新目录。
  4. 启动宿主版并观察启动日志，确认 migrations 正常。
  5. 记录迁移时间点与备份位置；未验证前禁止删除旧 DB。
  - 说明：此步骤不作为本轮迁移默认方案，仅留作未来扩展。

## 5. 部署步骤（容器 → 宿主 systemd）

1. **准备二进制与脚本**
   - 将 `pod-upgrade-trigger` 安装到 `~/.local/bin/pod-upgrade-trigger`（保持可执行）。
   - 前端已随 Release 二进制内嵌，无需额外同步 `web/dist`；如需覆盖 UI，可在 `${PODUP_STATE_DIR}/web/dist` 提供自定义构建，移除即可回退到内嵌版本。
   - 确认更新脚本存在：`/srv/pod-upgrade-trigger/update-pod-upgrade-trigger-from-release.sh`。
   - 确认自更新执行器：`/srv/pod-upgrade-trigger/self-update-runner.sh`。

2. **准备 host env**
   - 拷贝容器 env：`cp /srv/pod-upgrade-trigger/pod-upgrade-trigger.env ~/.config/pod-upgrade-trigger-http.env`。
   - 保留原有 `PODUP_STATE_DIR` / `PODUP_DB_URL` / `PODUP_PUBLIC_BASE` 等字段。
   - 在末尾补充自更新配置（参考 `systemd/pod-upgrade-trigger-http.env.example`）：
     - `PODUP_SELF_UPDATE_COMMAND=/srv/pod-upgrade-trigger/self-update-runner.sh`
     - `PODUP_SELF_UPDATE_CRON=0 */6 * * *`（示例）
     - `PODUP_SELF_UPDATE_DRY_RUN=1`（初始建议 dry-run）

3. **临时端口验证（避免 25111 冲突）**
   - 在宿主运行：
     - `PODUP_HTTP_ADDR=0.0.0.0:25211 pod-upgrade-trigger http-server`
     - 或 `pod-upgrade-trigger http-server --http-addr 0.0.0.0:25211`
   - 验证 `/health`、核心 API 与任务/事件查询；确认读取的仍是旧 DB（任务历史可见）。

4. **配置 user systemd unit**
   - 复制示例：`install -m 644 systemd/pod-upgrade-trigger-http.user.service.example ~/.config/systemd/user/pod-upgrade-trigger-http.service`。
   - 确认 `ExecStart=/home/<user>/.local/bin/pod-upgrade-trigger http-server`。
   - 将 `EnvironmentFile=` 指向 `~/.config/pod-upgrade-trigger-http.env`。
   - `systemctl --user daemon-reload`。

5. **切换到宿主版**
   - 停容器版 unit：`systemctl --user stop pod-upgrade-trigger.service`（或实际 Quadlet 名称）。
   - 确认 25111 释放：`ss -lntp | grep 25111` 应为空。
   - 启用并启动宿主版：`systemctl --user enable --now pod-upgrade-trigger-http.service`。
   - 复测 `/health`、webhook 流程与任务/事件可见性。

6. **启用自更新（先 dry-run 再实装）**
   - 保持 `PODUP_SELF_UPDATE_DRY_RUN=1` 运行多个调度周期，确认 `/tasks` 出现 `self-update` dry-run 记录且不替换二进制。
   - 准备切换到真实更新时，将 env 中的 `PODUP_SELF_UPDATE_DRY_RUN` 置空或设为 `0`，`systemctl --user restart pod-upgrade-trigger-http.service`。

## 6. 验证 checklist

- `/health` 返回 200。
- 关键 webhook 流程成功（例如升级回调触发）。
- `/tasks`、`/events` 能读取历史任务，host 版期间新任务正常写入。
- 自更新 dry-run 任务按 cron 周期出现，日志无异常 error/warn。
- `journalctl --user -u pod-upgrade-trigger-http.service` 无连续错误；25111 端口被宿主进程占用。

## 7. 回滚（宿主 → 容器）

1. 停宿主版：`systemctl --user stop pod-upgrade-trigger-http.service`。
2. 启动容器版 unit：`systemctl --user start pod-upgrade-trigger.service`（或实际名称）。
3. 确认 25111 被容器进程重新占用；`/health` 与上游 webhook 流程恢复正常。
4. 若宿主版运行期间触发了 DB schema 迁移，回滚前需确认容器版二进制兼容；必要时同时回退二进制版本。
5. 如需避免自更新在回滚后介入，可暂时在容器 env 中清空或禁用 `PODUP_SELF_UPDATE_*`。

## 8. 关联文档

- 设计与里程碑：`docs/host-systemd-release-deploy.md`
- 容器形态部署：`docs/container-deploy.md`
