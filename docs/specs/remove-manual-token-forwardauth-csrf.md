# 移除 Manual token：以 ForwardAuth + CSRF 保护手动触发接口

## 背景

当前 Web UI 顶栏提供 “Manual token” 输入框，前端会把该值写入 `localStorage` 并在 `POST` 请求里自动注入 `token` 字段。后端对 `/api/manual/*` 等“会触发真实动作”的接口，会在非 dev/demo 环境下要求 `token` 与服务端期望值匹配，否则返回 `401`。

主人选择的改动方向（B）是：**完全移除 Manual token 机制**，改为依赖 ForwardAuth（管理员头）进行鉴权，并补齐浏览器场景的 CSRF 防护。

同时，需要明确：**该方案不涉及 GitHub webhook 接收端点**（`/github-package-update/*`），webhook 仍然仅依赖 `PODUP_GH_WEBHOOK_SECRET` 的签名校验。

## 目标

1. 移除 Web UI 中的 Manual token 输入、存储与请求注入逻辑。
2. 所有手动触发/维护类“有副作用”的管理接口统一改为：
   - ForwardAuth（管理员头）鉴权；
   - CSRF 防护（自定义请求头 + JSON 限制）。
3. 生产环境 fail-closed：当 ForwardAuth 未正确配置时，管理接口不得默认放开。
4. webhook 接收端点保持不变：不引入 ForwardAuth/CSRF 依赖。

## 范围与非目标

### 范围

- 后端：
  - `/api/manual/*` 的 POST 入口增加 `ensure_admin` 校验；
  - 增加 `ensure_csrf` 并对“有副作用”的 admin API 强制启用；
  - 调整 ForwardAuth 的 open-mode 逻辑：生产环境缺失配置时不再放开；
  - 处理 legacy `/auto-update` 路由的迁移策略（见下文）。
- 前端：
  - 移除 token 相关 UI/状态管理；
  - 为所有“有副作用”的请求统一附加 CSRF 头；
  - 401 错误提示回归 ForwardAuth 未授权语义。
- 文档/测试：
  - 更新 env 示例、README、UI E2E/Rust E2E 相关用例，覆盖 ForwardAuth + CSRF 的新行为。

### 非目标

- 不引入新的登录系统/会话系统（Cookie/Session/OAuth）。
- 不改变 GitHub webhook 的签名校验与事件解析逻辑。
- 不新增 CORS 支持（保持默认同源即可）。
- 不在本工作项内实现多角色权限（只区分 admin 与非 admin）。

## 关键用例 / 用户流程

1. **管理员使用 UI 手动触发**
   - 用户通过反向代理/网关访问 UI；
   - 代理在请求中注入 ForwardAuth header；
   - UI 发起 `POST /api/manual/*`，自动携带 `X-Podup-CSRF`；
   - 后端通过 `ensure_admin` + `ensure_csrf` 后执行/调度任务。
2. **管理员使用脚本/CLI 调用手动 API**
   - `curl`/脚本显式携带管理员头（与服务端配置匹配），并添加 `X-Podup-CSRF`；
   - 后端按相同规则鉴权。
3. **GitHub webhook 投递**
   - GitHub 请求命中 `/github-package-update/*`；
   - 后端只验证 `PODUP_GH_WEBHOOK_SECRET`，不要求 ForwardAuth/CSRF。

## 设计概览

### 1) ForwardAuth（管理员头）模型

- 通过环境变量配置：
  - `PODUP_FWD_AUTH_HEADER`：用于识别身份的 header 名（后端会将其 lower-case 存入请求头 map）；
  - `PODUP_FWD_AUTH_ADMIN_VALUE`：匹配 admin 的固定值。
- 后端逻辑：当且仅当请求头 `PODUP_FWD_AUTH_HEADER` 的值等于 `PODUP_FWD_AUTH_ADMIN_VALUE` 时视为 admin。

**安全假设（必须成立）**

- 后端服务端口不可被不受信任来源直连；否则攻击者可自行伪造管理员头绕过鉴权。
- 反向代理必须清理/覆盖来自外部的同名 header，避免 header 注入。

### 2) CSRF 防护

对于“有副作用”的管理请求（POST/DELETE 等），增加以下校验：

- 必须携带自定义头：`X-Podup-CSRF: 1`（值可固定；关键在于“必须是自定义头”）。
- 对 JSON API 强制：`Content-Type` 需以 `application/json` 开头（防止 `text/plain` 的简单跨站投递）。

理由：

- 浏览器跨站请求无法在“简单请求”中携带自定义头；携带自定义头会触发 CORS 预检，而本服务不提供跨域放行，从而降低 CSRF 风险。
- 同源情况下（UI 自身）可正常发起带头请求。

> 备注：若未来引入跨域部署或显式 CORS，需要重新评估该策略。

### 3) 路由与策略矩阵（摘要）

| 路由 | 类型 | 是否需要 ForwardAuth | 是否需要 CSRF |
|---|---|---:|---:|
| `/github-package-update/*` | webhook 接收 | 否（签名校验） | 否 |
| `/api/manual/services` (GET) | 只读 | 是 | 否 |
| `/api/manual/*` (POST) | 有副作用 | 是 | 是 |
| `/api/tasks` (POST) / `/api/tasks/*` (POST) | 有副作用 | 是 | 是 |
| `/api/prune-state` (POST) | 有副作用 | 是 | 是 |
| `/api/image-locks/*` (DELETE) | 有副作用 | 是 | 是 |
| 其它 admin-only GET（events/settings/status 等） | 只读 | 是 | 否 |

### 4) legacy `/auto-update` 的处理

当前 `/auto-update` 属于“历史兼容的 token 触发路径”，且支持 GET/POST + query token。B 方案移除 token 后，建议：

- **方案默认（推荐）**：保留路由但改为 **POST-only**，并统一走 `ensure_admin + ensure_csrf`；
  - GET 返回 `405 MethodNotAllowed`（或 `410 Gone`）并提示使用 `/api/manual/auto-update/run` 或 UI。
- 好处：避免“误留一个无需管理员头的旧入口”；同时给现有脚本留一个迁移落点。

> 是否需要直接删除该路由（404/410）由主人最终拍板。

## 前端改动（概要）

1. 移除顶栏 Manual token 输入框与 `localStorage` 存储。
2. `POST/DELETE` 等有副作用请求统一增加请求头：
   - `X-Podup-CSRF: 1`
   - 保持 `Content-Type: application/json`
3. 401 行为回归 “ForwardAuth 未授权”：
   - 在生产环境继续跳转 `/401`；
   - 不再提示 “Manual token 缺失或错误”。

## 后端改动（概要）

1. `/api/manual/*` 的 POST 全部调用 `ensure_admin`。
2. 引入 `ensure_csrf`，并在所有“有副作用”的 admin API 上强制校验。
3. ForwardAuth fail-closed：
   - 非 dev/demo 环境下，当 `PODUP_FWD_AUTH_HEADER` 或 `PODUP_FWD_AUTH_ADMIN_VALUE` 缺失时，admin API 返回错误并提示需要配置（避免 silent open）。
4. token 相关字段与环境变量：
   - `PODUP_MANUAL_TOKEN`：废弃（不再读取，不再在 settings/UI 展示）。
   - `PODUP_TOKEN`：从“手动触发鉴权”中剥离；若仅用于 legacy `/auto-update`，则随该路由策略一起迁移/废弃。

## 兼容性与迁移

- UI 行为变化：不再要求输入 token；依赖反向代理正确注入管理员头。
- API 变化：
  - `/api/manual/*` 请求体不再需要 `token` 字段（可在过渡期“接受但忽略”，避免外部调用立刻崩）。
  - 所有有副作用请求必须带 `X-Podup-CSRF`。
- 部署变化：
  - 生产环境需要显式配置 `PODUP_FWD_AUTH_HEADER` + `PODUP_FWD_AUTH_ADMIN_VALUE`；
  - 确保后端端口不对公网直露，并由反代清理同名 header。

## 风险与缓解

1. **后端可直连导致管理员头被伪造**
   - 缓解：网络层隔离（只允许反代访问后端）、iptables/防火墙、或将后端绑定到 loopback。
2. **未来启用 CORS 跨域导致 CSRF 防护失效**
   - 缓解：保持默认同源；如需跨域，改用双提交 Cookie 或一次性 CSRF token。
3. **生产环境误入 open 模式**
   - 缓解：`PODUP_ENV=prod` 下强制 fail-closed，并在 `/api/settings` 返回“ForwardAuth 未配置”的显式信号。

## 测试计划（概要）

- Rust E2E：
  - admin header 缺失时，`/api/manual/*` 返回 401；
  - admin header + 缺 CSRF 头时，返回 403/400；
  - admin header + CSRF 头时，手动触发成功；
  - webhook 端点不受影响（仍按签名规则）。
- UI E2E：
  - 不再出现 Manual token 输入框；
  - manual trigger/auto-update run 的成功路径覆盖；
  - 401 仍能正确跳转到 `/401`。

## 待确认项

1. `/auto-update`：保留并改造（POST-only + admin + CSRF），还是直接移除（404/410）？
2. CSRF 失败返回码：建议 `403 Forbidden`（或 `400 BadRequest`），主人偏好哪种？
3. 生产环境判定：是否以 `PODUP_ENV` 为准（推荐），还是仅依赖 `PODUP_DEV_OPEN_ADMIN` 开关？

