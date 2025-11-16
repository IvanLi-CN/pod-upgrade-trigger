# 容器化部署可行性分析

本文评估将当前项目以容器方式运行的可行性、风险与推荐实践，结合现有代码/工作流现状（commit e754aaa）。

## 结论概览

- 可行，但需要额外工作：镜像需内置 `podman-remote`，并挂载宿主 Podman socket；待程序提供内建 HTTP 监听后才能直接对外服务。
- 对于以宿主 systemd + Podman 为核心的场景，容器需要 `--net host` + 挂 socket + 同 UID/GID 权限，部署复杂度高于直接在宿主运行二进制。
- 若短期不做镜像改造或 HTTP 监听尚未落地，建议继续宿主运行（或走 AUR/Release 包）。

## 当前阻点

1) **缺少 HTTP 常驻监听**：程序以 inetd/socket 方式从 stdin 处理单次请求；容器虽可 `EXPOSE 8080`，但没有监听逻辑，需未来改成自带 HTTP server 后才可直接 `--net host` 对外服务。
2) **依赖宿主 Podman/systemd**：
   - 代码内部依赖 `podman pull`、`systemd-run` 等命令驱动宿主容器和 systemd unit。
   - 容器内需要 `podman-remote`（或完整 podman）并挂载宿主 Podman socket，设置 `PODMAN_HOST=unix:///run/podman/podman.sock`（或 rootless 路径 `/run/user/$UID/podman/podman.sock`）。
3) **权限与状态目录**：
   - 状态/锁文件路径可通过 `WEBHOOK_STATE_DIR` 自定义并挂载到容器；需确保 UID/GID 与宿主一致，避免权限问题。
   - 访问前端静态资源时，`WEBHOOK_WEB_DIST` 需要指向镜像内或挂载的 dist 目录。

## 镜像侧的改造建议

- **在 Dockerfile 安装 podman-remote**：
  ```Dockerfile
  RUN apt-get update \
      && apt-get install -y --no-install-recommends podman-remote \
      && rm -rf /var/lib/apt/lists/*
  ```
- 保留现有多阶段构建（Bun 构建前端，Rust release 二进制）。
- 为未来 HTTP 监听预留入口，例如 `CMD ["webhook-auto-update", "--http-bind", "0.0.0.0:8080"]`，并在添加监听后更新。

## 推荐运行参数（待 HTTP 监听就绪后）

示例（root 场景，宿主 Podman socket：`/run/podman/podman.sock`）：

```bash
podman run -d --name pod-upgrade-trigger \
  --network host \
  -e WEBHOOK_STATE_DIR=/srv/webhook/data \
  -e WEBHOOK_WEB_DIST=/srv/webhook/web \
  -e PODMAN_HOST=unix:///run/podman/podman.sock \
  -v /srv/webhook/data:/srv/webhook/data:Z \
  -v /srv/webhook/web:/srv/webhook/web:Z \
  -v /run/podman/podman.sock:/run/podman/podman.sock:Z \
  ghcr.io/ivanli-cn/pod-upgrade-trigger:latest \
  /usr/local/bin/webhook-auto-update --http-bind 0.0.0.0:8080
```

Rootless 场景：将 socket 改为 `/run/user/$UID/podman/podman.sock`，容器需以同 UID 运行（`--user $(id -u):$(id -g)`），并确保挂载目录的属主一致。

## 与直接在宿主运行的对比

**优点**
- 易于回滚/版本锁定：拉取指定 tag 镜像即可切换版本。
- 依赖收敛：镜像内包含 Bun/Rust 构建产物，宿主无需安装构建链。

**缺点/风险**
- 必须暴露宿主 Podman socket（或 rootless socket），有安全面；需确认最小权限和适当的 socket ACL。
- `systemd-run` 等调用依赖宿主 systemd 环境，容器需要 `--net host` 且可能追加 `--pid host`/`--ipc host`（视后续实现而定）。
- 部署复杂度高于宿主二进制：需要处理 UID/GID、SELinux 上下文（如 `:Z`）、挂载点和 env。
- 在 HTTP 监听落地前，仍需外部 socat/inetd 包装容器才能收请求，不推荐。

## 推荐路径

短期：
- 保持宿主运行二进制（或 AUR/Release 安装），等待内建 HTTP 监听落地。
- CI 发布镜像时先实验性加入 `podman-remote`，但不在生产启用。

中期（HTTP 监听完成后）：
- 更新 Dockerfile 安装 `podman-remote`，在 README/部署文档中给出 `PODMAN_HOST`、`--net host`、socket 挂载示例。
- 提供 systemd unit 包装 `podman run`，统一管理容器生命周期。

长期：
- 若需要降低 socket 暴露风险，可考虑提供最小化的 “控制端” 子进程，只负责验证事件并通过受限 RPC 指示宿主做更新，减少直接操纵 Podman socket 的必要。
