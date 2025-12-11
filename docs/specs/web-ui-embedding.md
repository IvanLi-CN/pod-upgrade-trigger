# Web 前端静态资源内嵌设计

本设计文档描述如何将 `web/dist` 前端构建产物内嵌到 `pod-upgrade-trigger` 二进制中，并调整静态资源加载策略，以支持在 host systemd 部署场景下仅依赖单个二进制与必要环境变量即可提供完整 Web UI。

## 背景与动机

当前在 host systemd 部署形态下，后端 HTTP 服务可以正常提供 `/health`、`/auto-update`、`/api/**` 等接口，但访问 `/` 时会返回：

- HTTP 500 `InternalServerError`；
- body 为固定字符串 `"web ui not built"`。

导致该行为的原因是：

- `try_serve_frontend` 仅从一系列磁盘路径寻找前端静态资源；
- 实际运维环境只分发后端二进制与 env，而不会额外同步 `web/dist`；
- 因此在没有磁盘 dist 的情况下，根路径 UI 无法使用。

运维侧有明确要求：在 host systemd 形态下，需要“单二进制 + env”即可完成部署，不应额外依赖同机同步的前端构建产物。这一要求需要通过在二进制中内嵌前端静态资源来满足。

## 目标与非目标

### 目标

- 在 release 构建过程中，将 `web/dist` 目录中的前端构建产物内嵌到 `pod-upgrade-trigger` 二进制中；
- 调整静态资源加载策略：
  - 优先从现有磁盘路径（含 `PODUP_STATE_DIR`、当前工作目录、构建目录、`/srv/app/web` 等）读取；
  - 当磁盘路径全部不可用或缺失目标文件时，从内嵌资源中读取；
  - 仅在磁盘与内嵌资源都不完整的特殊构建形态下，才返回 `"web ui not built"`；
- 在 host systemd 部署场景（`WorkingDirectory` 指向 state 目录，仅包含 DB/日志等）下，即使不存在 `web/dist` 目录，也能通过访问 `/`、`/events`、`/tasks`、`/settings` 等前端路由正常展示 UI；
- 保持现有 HTTP API 行为和响应格式不变，仅调整静态资源的加载来源；
- 控制二进制体积增加在可接受范围内（以当前 `web/dist` 体积预计增加约 +1–2 MB）。

### 非目标

本次改动不包含以下内容：

- 不调整前端路由结构和页面交互逻辑（参见 `docs/web-ui-design.md`）；
- 不变更 `/health`、`/api/**`、`/github-package-update/**` 等现有 HTTP 接口的语义或返回格式；
- 不在首个版本中引入复杂的压缩/预压缩/按需解压方案；
- 不对现有 UI E2E 测试框架或脚本 (`web/tests/ui/*.spec.ts`、`scripts/test-ui-e2e.sh`) 做结构性重构。

## 现有行为概览

### 前端路由与静态资源托管

当前后端通过 `try_serve_frontend` 处理与 UI 相关的路由。该函数的行为要点如下：

- 仅处理 `GET` 和 `HEAD` 请求；
- 将以下路径统一映射为前端入口 `index.html`：
  - `/`、`/index.html`、`/manual`、`/webhooks`、`/events`、`/tasks`、`/maintenance`、`/settings`、`/401`；
- `/assets/**` 通过 `sanitize_frontend_path` 去掉前导斜杠并清洗路径后映射到静态文件；
- 特殊静态文件路径：
  - `/mockServiceWorker.js` → `mockServiceWorker.js`；
  - `/vite.svg` → `vite.svg`；
  - `/favicon.ico` → `favicon.ico`。

对所有上述路径，当前实现都会调用 `frontend_dist_dir()` 选择一个磁盘目录，然后拼接出 `asset_path = dist_dir.join(relative)` 并尝试读取文件。

### 静态资源磁盘路径查找

`frontend_dist_dir()` 的职责是从一组固定候选路径中选择用于查找前端资源的目录。当前候选顺序为：

1. `${PODUP_STATE_DIR}/web/dist`（如配置了 `PODUP_STATE_DIR` 且非空）；
2. `${WorkingDirectory}/web/dist`（进程当前工作目录）；
3. `${CARGO_MANIFEST_DIR}/web/dist`（构建时的 crate 根目录下的 `web/dist`）；
4. `/srv/app/web`（默认的系统级部署路径）。

实现上：

- 优先返回第一个 `is_dir()` 为 true 的候选目录；
- 若未找到任何存在的候选目录，则回退到候选列表中的第一个路径（即使目录尚不存在）。

### 错误响应行为

- 若 `asset_path` 是存在的普通文件：
  - 对 `HEAD`：读取 metadata 获取长度，返回 200；
  - 对 `GET`：读取文件内容并返回 200；
- 若 `relative` 指向 `index.html` 且文件不存在：
  - 记录日志 `"500 web-ui missing index.html"`；
  - 返回 500，body `"web ui not built"`；
- 对其他静态资源路径（例如 `/assets/**`）：
  - 文件缺失时返回 404，body `"asset not found"`。

在 host systemd 场景中，由于通常不会同步 `web/dist`，`frontend_dist_dir()` 找不到任何实际存在且包含 `index.html` 的目录，最终导致访问 `/` 返回 500。

## 设计方案概述

本设计在不改变现有 HTTP API 语义的前提下，引入内嵌前端资源并调整静态资源加载顺序。

### 依赖与新模块

- 在 `Cargo.toml` 中新增 `rust-embed` 依赖，用于在编译期将 `web/dist` 目录内容打包进二进制；
- 在后端代码中定义一个内嵌资源类型，例如：

  ```rust
  #[derive(RustEmbed)]
  #[folder = "web/dist"]
  struct EmbeddedWeb;
  ```

- 内嵌资源类型对其它模块暴露一个简单接口：

  ```rust
  impl EmbeddedWeb {
      fn get_asset(path: &str) -> Option<Cow<'static, [u8]>>;
  }
  ```

  该接口内部直接调用 `EmbeddedWeb::get(path)`，并将返回值包装为合适的类型。

### 加载顺序与兜底策略

在设计上，静态资源加载遵循如下顺序：

1. **磁盘优先**：
   - 保留并复用现有的 `frontend_dist_dir()` 逻辑，用于决定优先使用哪个磁盘 `web/dist` 目录；
   - 对每个请求，仍先构造 `asset_path = dist_dir.join(relative)`；
   - 若 `asset_path.is_file()` 为 true，则使用当前逻辑从磁盘读取并返回 200。

2. **内嵌兜底**：
   - 当 `asset_path.is_file()` 为 false 时，引入内嵌资源回退：
     - 将 `relative` 转换为不带前导斜杠的字符串 `rel_str`；
     - 调用 `EmbeddedWeb::get_asset(rel_str)` 尝试从内嵌资源中获取内容：
       - 如命中，对 `HEAD` 请求使用 `data.len()` 作为长度调用 `respond_head`；
       - 对 `GET` 请求使用 `respond_binary` 返回 body；
     - 日志与事件记录中的 `"asset"` 字段保持使用 `relative` 的字符串表示。
   - 对入口路由（所有映射到 `index.html` 的路径），若第一轮 `rel_str` 查找失败：
     - 额外再尝试一次 `EmbeddedWeb::get_asset("index.html")`，以防未来路径映射逻辑调整时出现字符串差异。

3. **最终失败与错误响应**：
   - 当目标是入口 `index.html`，且磁盘与内嵌查找都失败时：
     - 保留并复用现有的 500 行为，返回 `"web ui not built"`，日志中继续打印 `"500 web-ui missing index.html"`；
   - 对其他静态资源（如 `assets/*.js`）：
     - 如磁盘与内嵌都无法命中，对现有 404 行为不做改变，继续返回 `"asset not found"`。

### 对外行为变化

在上述设计下，对外可观察到的行为变化主要集中在以下几点：

- 在 host systemd 部署但没有任何磁盘 `web/dist` 时：
  - 访问 `/`、`/index.html`、`/manual`、`/webhooks`、`/events`、`/tasks`、`/maintenance`、`/settings`、`/401` 将不再返回 500，而是返回 200 并提供内嵌的 `index.html`；
  - 浏览器随后从同一路径下加载内嵌的 JS/CSS/图片等资源，UI 可用；
- 在开发环境、带磁盘 dist 的测试环境：
  - 优先使用磁盘 `web/dist`，行为与现状保持一致；
  - 内嵌资源只在磁盘完全缺失时参与兜底；
- 对 `/health`、`/auto-update`、`/api/**`、`/github-package-update/**` 等接口：
  - 请求路径不会进入 `try_serve_frontend`，行为保持不变。

## 构建流程与约束

### Release 构建前置条件

为了确保 release 二进制中始终包含最新的前端资源，本设计要求在构建 release 目标前已经构建好 `web/dist`，推荐的流水线步骤为：

1. 在仓库根目录或 CI 脚本中显式执行前端构建：
   - 例如：

     ```bash
     cd web
     bun install --frozen-lockfile || bun install
     bun run build
     cd ..
     ```

2. 返回仓库根目录后，执行：

   ```bash
   cargo build --release --bin pod-upgrade-trigger
   ```

release 构建将通过 `rust-embed` 把 `web/dist` 目录打包进二进制。

### 构建期校验（可选但推荐）

为避免误构建“未内嵌前端”的 release 二进制，可增加轻量级 `build.rs` 执行以下校验：

- 检查 `${CARGO_MANIFEST_DIR}/web/dist/index.html` 是否存在；
- 若不存在，则在编译阶段失败，并打印指引信息（例如：要求先在 `web/` 目录下执行前端构建命令）。

这种方式可以在 CI 与本地构建中尽早暴露配置错误。

### 开发与测试模式

- 开发脚本 `scripts/dev-server.sh` 当前已经会在缺少 `web/dist` 时尝试构建前端 bundle，可保持原有逻辑；
- UI E2E 脚本 `scripts/test-ui-e2e.sh` 会在 debug 构建前端并使用磁盘版本；在内嵌方案落地后：
  - 在正常路径下仍优先使用磁盘 dist；
  - 内嵌资源不会影响现有 UI E2E 测试流程。

## 兼容性与迁移分析

### 向后兼容性

- HTTP API：
  - 所有现有 JSON API（`/health`、`/auto-update`、`/api/**` 等）保持路径与返回格式不变；
  - 不新增或修改与前端内嵌直接相关的环境变量；
  - 原有 host systemd 部署中，为了获得 UI 而手动同步的 `web/dist` 仍然被优先使用。

- 部署与运维：
  - 对于原先只分发二进制而不分发前端的主机，新版本部署后 `/` 将从 500 变为 200 + UI，可以视为增强行为；
  - 对于已有磁盘 UI 覆盖（例如在 state 目录放置定制 `web/dist`）的实例，行为不变，仍优先读取磁盘版本。

### 潜在行为变化

唯一需要注意的行为变化是：不再存在“默认 host 部署访问 `/` 必然 500”这一现象。如果有外部系统错误地把 `/` 的 500 当作“后端未部署 UI”的信号，这种用法将在新版本中不再成立。合理的健康检查与部署验证应当依赖 `/health`。

## 风险与缓解措施

### 风险点

- 构建流水线未正确构建 `web/dist`：
  - 若不进行构建期校验，可能导致 release 二进制中的内嵌前端为空，回退行为仍然是 500 `"web ui not built"`；
- 内嵌资源体积增长：
  - 随着前端功能增加，`web/dist` 体积可能显著增大，导致二进制大小增加较多；
- 新增依赖的兼容性：
  - `rust-embed` 版本需要与当前 Rust 版本和依赖树兼容，需在引入时确认。

### 缓解措施

- 构建期校验：
  - 推荐增加 `build.rs` 检查 `web/dist/index.html` 是否存在，从源头防止“无内嵌 UI 的 release”；
- 体积监控：
  - 在引入内嵌功能的 MR 中记录“前后二进制大小对比”；
  - 若后续 `web/dist` 体积出现明显增加，再评估是否启用 `rust-embed` 的压缩特性或引入预压缩静态文件方案；
- 回退与日志：
  - 保留 `"500 web-ui missing index.html"` 日志，用于快速识别“磁盘与内嵌都缺失”的异常构建；
  - 如有需要，可在未来迭代中在审计事件 meta 中记录资源来源（磁盘/内嵌），帮助排查问题。

## 测试与验收计划

为验证内嵌方案的正确性与兼容性，建议至少覆盖以下测试场景：

### 1. 构建与基础访问验证

- 使用与 Release 相同配置构建二进制（先构建 `web/dist`，再 `cargo build --release`）；
- 在一个仅包含 state 目录的空工作目录中运行：

  ```bash
  PODUP_STATE_DIR=/tmp/podup-state \
    pod-upgrade-trigger http-server
  ```

- 验证：
  - `curl http://127.0.0.1:25111/health` 返回状态 `"ok"`；
  - `curl http://127.0.0.1:25111/` 返回 HTTP 200，响应体包含 `<html>` 且不包含 `"web ui not built"`；
  - 可以选取 `/events` 等前端路由验证入口路由都能返回 HTML。

### 2. 磁盘覆盖优先级验证

- 在同一进程下：
  - 在 `${PODUP_STATE_DIR}/web/dist/index.html` 写入一个带明显标记的 HTML 内容；
  - 访问 `/`，确认返回的是磁盘版本；
  - 删除 `${PODUP_STATE_DIR}/web/dist` 后再次访问 `/`，确认回退到内嵌版本且仍然返回 200。

### 3. 回归测试

- Rust 层：
  - 执行现有 `cargo test` 与 E2E 套件（如有脚本 `scripts/test-e2e.sh`），确认与前版本一致；
- UI 层：
  - 使用 `scripts/test-ui-e2e.sh` 启动后端并运行 Playwright 测试；
  - 确认 `/manual`、`/webhooks`、`/events`、`/maintenance`、`/settings` 等页面在“磁盘优先”模式下依然可用；
  - 如有需要，追加一套“只依赖内嵌资源”的轻量级 UI 回归检查。

### 4. 体积与构建时间记录

- 在引入内嵌功能的 MR 中记录：
  - release 二进制改动前后的体积对比（例如：`du -sh target/release/pod-upgrade-trigger`）；
  - 构建时间变化的粗略估计（可选，仅作说明）。

## 总结

通过引入 `rust-embed` 将 `web/dist` 内嵌进二进制，并在 `try_serve_frontend` 中实现“磁盘优先、内嵌兜底”的加载策略，本设计可以在保持现有 API 完全兼容的前提下，使 host systemd 部署在只分发单一二进制与必要环境变量时也能提供完整 Web UI。后续实现阶段可严格按照本文档约束的行为、构建约束和测试计划推进。

