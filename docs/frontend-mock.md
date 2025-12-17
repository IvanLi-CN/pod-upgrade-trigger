# Frontend Mock Strategy

## 1. 目标

- 前端在无后端进程的情况下完整运行与演示。
- Mock 返回体与真实后端接口形状一致，随时可切换场景。
- 开关明确，默认不影响生产构建与真实联调。

## 2. 技术选型

- `msw@2` browser worker：拦截 `fetch` 与 `EventSource`。
- `@faker-js/faker`：可选，用于生成动态数据。
- `lodash-es`：数据拼装与克隆（可选）。
- `zod`：可选的返回体校验，避免“假数据成功、真接口失败”。
- 开启条件：`VITE_ENABLE_MOCKS=true` 或 URL 查询含 `mock`。

## 3. 目录结构（建议）

```
web/
  src/mocks/
    browser.ts       # setupWorker，按开关启动
    server.ts        # Node/Playwright 复用（如需）
    handlers.ts      # 统一注册 handlers
    runtime.ts       # 场景管理、内存 store、localStorage 持久
    data/
      events.ts
      manual.ts
      webhooks.ts
      settings.ts
  public/
    mockServiceWorker.js  # npx msw init public --save 生成
    fixtures/last_payload.bin (可选示例)
```

## 4. 覆盖接口矩阵

- GET `/health`
- GET `/sse/hello`（单次 hello，可模拟错误）
- GET `/api/settings`
- GET `/api/events`（分页 + request_id/path_prefix/status/action 过滤）
- GET `/api/tasks`（分页 + status/kind/unit 过滤）
- GET `/api/tasks/:id`（任务详情 + 日志）
- POST `/api/tasks`（创建临时/长耗时任务，返回 `task_id`）
- POST `/api/tasks/:id/stop`（优雅停止任务）
- POST `/api/tasks/:id/force-stop`（强制停止任务）
- POST `/api/tasks/:id/retry`（从终态任务创建重试任务）
- GET `/api/manual/services`
- POST `/api/manual/deploy`
- POST `/api/manual/services/:slug`
- POST `/api/manual/auto-update/run`
- POST `/api/manual/trigger`（legacy：restart-only，兼容保留）
- GET `/api/webhooks/status`
- GET `/api/image-locks`
- DELETE `/api/image-locks/:bucket`
- GET `/api/config`
- POST `/api/prune-state`
- GET `/last_payload.bin`

## 5. 场景预设

- `happy-path`：健康、数据齐全。
- `empty-state`：各列表为空，验证空态。
- `rate-limit-hot`：事件与锁偏多，用于高负载演示。
- `auth-error`：主要接口返回 401，触发前端授权处理。
- `degraded`：/health 与 SSE 失败，验证异常态。

切换方式：URL `?mock=profile=rate-limit-hot` 或在 mock 模式下渲染的“Mock 控制台”浮层；浮层需提供场景切换、延迟/错误率调节、数据重置。

## 6. 运行时约定

- 在 `src/main.tsx` 顶部按需启动：

  ```ts
  if (import.meta.env.VITE_ENABLE_MOCKS === 'true' || window.location.search.includes('mock')) {
    await import('./mocks/browser').then(({ worker }) =>
      worker.start({ onUnhandledRequest: 'bypass' })
    )
  }
  ```

- `runtime` 维护当前 profile、可变数据（触发历史、锁列表等），操作后实时更新页面。
- 时间戳保持秒级，与现有页面类型一致；必要时使用 `zod.safeParse` 保障字段一致性。

## 7. 落地步骤

1) 安装依赖（web 目录）：`npm i -D msw @faker-js/faker lodash-es zod`。
2) 生成 worker：`npx msw init public --save`。
3) 创建 `src/mocks` 目录与基础文件，补齐 handlers 覆盖矩阵。
4) 在 `src/main.tsx` 增加按需启动逻辑；`package.json` 新增脚本：
   - `dev:mock`: `VITE_ENABLE_MOCKS=true vite`
   - `preview:mock`: `VITE_ENABLE_MOCKS=true vite preview`
5) 实现 Mock 控制台（仅 mock 模式渲染），支持切换场景与重置数据。
6) Playwright/E2E：新增“mock happy-path”用例访问 Dashboard/Services/Webhooks/Events 并断言渲染。
7) 自测：`npm run dev:mock`，确认所有页面无后端也可工作；记录未覆盖接口并补齐。

## 8. 验证清单

- [ ] 无后端进程时，全部页面加载正常。
- [ ] `/tasks` 页面在 mock 模式下可以正常列出任务、打开详情抽屉，并通过 stop/force-stop/retry 按钮更新状态与时间线。
- [ ] 场景切换后数据与状态同步变化；空态/异常态表现正确。
- [ ] 未实现接口会在控制台提示，但不阻断页面。
- [ ] E2E/CI 可在 mock 模式稳定运行。

## 9. 可选增强

- 录制真实接口响应生成 fixtures，用于对比差异。
- 在 Mock 控制台加入“网络延迟/错误率”滑杆，覆盖 Loading/Retry 流程。
- 将所选 profile 持久化到 localStorage，刷新后保持一致。
