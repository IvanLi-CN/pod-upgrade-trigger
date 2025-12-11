# 测试分层与 Storybook 方案

## 1. 背景与目标

`pod-upgrade-trigger` 目前已经具备：

- Rust 级别的单元 / 集成测试；
- 基于 mock 二进制与临时 SQLite 的后端 e2e 测试（见 `docs/e2e-testing.md`）；
- 通过 Playwright 的 web UI e2e 测试（`web/tests/ui`，以及 `scripts/test-ui-e2e.sh` / `scripts/test-ui-e2e-mock.sh`）。

这些测试主要覆盖“端到端行为路径”，对组件级状态、交互与视图细节的覆盖不够细腻。本方案的目标是：

- 为前端引入 Storybook，提供组件与页面的隔离开发环境；
- 在 Storybook 之上增加自动化测试能力，用于补足“组件 / 视图层”的行为验证；
- 明确各层测试的职责边界和推荐实践，形成统一的“测试金字塔”。

## 2. 范围与非目标

本次工作范围：

- 在 `web/` 子项目中集成 Storybook（React + Vite 框架）；
- 为核心组件 / 页面补充首批 stories（含多种状态与错误分支）；
- 基于 Storybook 8.6 提供的 experimental addon-test + Vitest 3（browser mode）增加组件级自动化测试；
- 在项目文档中梳理整体测试分层与 Storybook 的位置。

明确不在本次范围内的内容：

- 不改动后端 Rust 服务的 API 设计、数据库 schema 或任务调度逻辑；
- 不替换或弱化现有 Playwright e2e 套件，后者仍然用于关键用户路径回归；
- 不在本阶段引入基于截图的视觉回归测试（可作为后续增强选项）。

## 3. 测试分层总览

整体测试策略按“越靠近逻辑层，测试越多”的原则分为四层：

1. Rust 层
   - 单元测试：针对纯业务逻辑和小函数，验证边界条件与错误分支。
   - 集成 / e2e 测试：通过 mock bin 与临时 SQLite，覆盖核心任务链路（详见 `docs/e2e-testing.md`）。
2. 前端组件层（本次新增）
   - Storybook + Vitest：围绕 `web/src/components` 与 `web/src/ui` 等组件，验证界面状态切换、属性组合、表单校验与用户交互。
   - 利用 MSW / mock runtime 构造丰富的前端数据场景。
3. 前端页面与路由层
   - 通过 Storybook 中的“页面级” stories 或轻量交互测试，覆盖局部页面逻辑（如任务详情抽屉、事件筛选器）。
4. 端到端 UI 层
   - 继续使用 `web/tests/ui/*.spec.ts` + `scripts/test-ui-e2e*.sh` 进行真实后端 / mock 后端驱动的 e2e。

Storybook 组件测试位于第 2–3 层，承担“比 e2e 更细粒度、比纯单元更贴近 UI”的职责。

## 4. 关键用例与用户流程

围绕现有前端功能，本方案优先覆盖以下 Storybook 用例：

- 任务中心（Tasks）
  - 展示不同任务类型：手动任务、GitHub Webhook 任务、调度器任务、维护任务等；
  - 展示不同任务状态：运行中、成功、失败、取消、未知；
  - 时间线视图中含命令级日志与命令输出折叠（包括 stdout/stderr）。
- 事件列表（Events）
  - 正常列表、空列表、速率限制“热点”场景；
  - 筛选条件与分页状态。
- 手动触发面板（Manual）
  - 自动更新、单服务触发、多服务批量触发；
  - 表单校验错误、后端失败、成功反馈等状态。
- Webhooks 与 Settings
  - Webhook 单元的 HMAC OK / 失败状态；
  - ForwardAuth 为 open / protected 模式，Settings 页面对应展示差异。

对于上述用例，Storybook 提供：

- 交互式浏览：开发者可在 Storybook 中切换不同场景、调试样式与文案；
- 行为测试入口：为关键 stories 编写基于 Vitest 的交互测试，验证表单提交、按钮可用性、状态切换等。

## 5. 数据与领域模型

本次方案不涉及后端数据库 schema 变更，也不调整 Rust 领域模型。前端侧仅在以下方面扩展：

- 继续复用 `web/src/domain/**` 中的 TypeScript 类型作为 Storybook stories 的 props 与数据模型；
- 在 `web/src/mocks` 或新的 `web/src/testing/fixtures.ts` 中，抽取部分可复用的前端数据构造函数，例如：
  - 构造不同状态的 `Task` / `TaskLogEntry`；
  - 构造带有 HMAC 状态与锁信息的 webhook 列表；
  - 构造 Settings / Locks / Events 的典型快照。

这些 fixtures 仅服务于前端 stories 与组件测试，不改变任何后端持久化结构。

## 6. 接口与模块边界设计

### 6.1 Storybook 基础集成

- 在 `web/` 项目中引入 Storybook（React + Vite 框架），新增：
  - `.storybook/main.ts`：配置 stories 匹配规则（例如 `../src/**/*.stories.@(ts|tsx)`）、启用必要的官方 addons（如 actions、controls、interactions）以及 Vitest 集成；
  - `.storybook/preview.tsx`：配置全局 decorators 和参数，包括：
    - Tailwind / DaisyUI 主题与全局样式（重用 `web/src/index.css`）；
    - React Router 容器（必要时为页面级 stories 提供路由上下文）；
    - MSW / mock runtime 的全局初始化（详见下一小节）。
- 在 `web/package.json` 中增加 Storybook 相关脚本，便于本地开发、构建和运行组件测试。

### 6.2 Mock 与 MSW 集成

当前项目已经在 `web/src/mocks` 中实现：

- `runtime.ts`：前端 mock 运行时，提供 tasks/events/settings 等丰富数据；
- `handlers.ts`：MSW handlers，与后端 API 形状保持一致；
- `browser.ts`：浏览器侧 Service Worker 启动逻辑。

在 Storybook 中将复用上述设施：

- 在 `preview.tsx` 中初始化 MSW / runtime，使 stories 与 `dev:mock` / `preview:mock` 的行为尽量一致；
- 为不同 stories 选择合适的 mock profile（如 `happy-path`、`empty-state`、`rate-limit-hot` 等），体现真实环境下的多种状态；
- 对于仅依赖 props、不依赖网络的“纯组件”，可以用 fixtures 直接构造数据，避免过度依赖 MSW。

### 6.3 stories 组织方式

- 为基础 UI 和通用组件优先编写 stories，例如：
  - 表格 / 列表组件；
  - 任务时间线组件；
  - 事件条目 / 提示组件；
  - Toast / Badge / Tag 等状态组件。
- 页面级 stories：
  - 对 `web/src/pages` 中的关键页面（Tasks / Events / Manual / Settings）提供一到两种“典型场景”；
  - 页面 stories 中通过 Router 与 MSW 组合模拟路由与数据。
- 命名与路径：
  - 按目录结构归类，例如 `components/tasks/TaskTimeline.stories.tsx`；
  - 使用清晰的 story 名称（如 “Succeeded task with command logs”），便于搜索与回归。

### 6.4 自动化测试集成（Vitest + Storybook）

当前实现（Storybook 8.6.x）：

- 技术栈：Storybook 8.6.14（React + Vite），`@storybook/experimental-addon-test` + `@storybook/test` + Vitest 3 + `@vitest/browser`。
- 运行模式：Vitest browser mode，provider 使用 Playwright（Chromium），对应配置见 `web/vitest.storybook.config.ts`。
- 现有覆盖：已为 `AutoUpdateWarningsBlock` 与 `Toast` 编写首批组件级测试（基于 stories 与 fixtures）。

本地与 CI 运行命令：

- 本地：`cd web && bun run test:storybook`（或使用 npm：`npm run test:storybook`）。
- CI：在 `.github/workflows/ci.yml` 中新增 `storybook-tests` job（Ubuntu + Bun 1.3.3 + Playwright 浏览器），依赖 `lint`、`unit-tests`，执行 `bun run test:storybook`。

前瞻（Storybook 10）：若升级到 Storybook 10，可考虑改用正式的 `@storybook/addon-vitest`；届时需要重新验证 Vite/React 版本兼容性与配置（browser provider、MSW 复用、fixtures），再调整 `vitest.storybook.config.ts` 与 CI 脚本。

边界：Storybook 组件测试聚焦前端行为与交互，不对后端调用结果做断言；后端行为仍由 Rust e2e 与 UI e2e 负责。

## 7. 兼容性与迁移考虑

- 新增依赖均为前端 devDependencies，不会影响后端编译与运行；
- Storybook 配置将尽量复用现有 Vite 与 TypeScript 配置，不改变 `web` 的生产构建输出；
- Playwright e2e 测试脚本与 `scripts/test-ui-e2e*.sh` 不需要修改；
- CI 层面增加 Storybook 组件测试 job 时，应注意：
  - 控制执行时间，避免与现有 Rust / Playwright job 叠加后超出合理时长；
  - 如有需要，可通过环境变量控制是否在本地开发时启用 Storybook 测试。

## 8. 风险与待确认问题

本方案存在的主要风险与待确认点包括：

- Storybook 版本与 React 19 / Vite 7 组合的兼容性：需要在实现阶段选定并验证官方推荐的版本组合；
- Vitest 集成模式：当前采用 Storybook 8.6 的 experimental addon-test（基于 Vitest 3 browser + Playwright）。若未来切换到 Storybook 10，则需改用正式的 addon-vitest 并重新验证兼容性。
- mock 一致性：需要约定 stories 中优先复用 `web/src/mocks` 与 fixtures，避免散落多份“半相同”的假数据；
- 覆盖范围：首批 stories 与组件测试具体覆盖哪些组件 / 页面，需要结合现有 UI e2e 用例优先级，由维护者进一步排序。

在上述问题确认后，可基于本设计逐步实现 Storybook 集成与测试脚本，并在 CI 中上线对应的组件级自动化测试。

## 9. 可行性结论

- 技术栈匹配：experimental addon-test 针对 Vite/React 场景设计，当前 React + Vite + TypeScript 组合已在本仓库验证通过；未来如迁移到 Storybook 10，可按官方 addon-vitest 重新校验。
- 影响范围可控：所有改动限定在 `web/` 子项目（引入 Storybook 依赖、`.storybook` 配置、stories 与组件级测试），对 Rust 后端、SQLite 迁移以及现有 Playwright e2e 流水线均为“零侵入”，只需在 CI 中新增一个前端组件测试 job。
- 渐进式落地：可以先完成 Storybook 基础接入与 1–2 个代表性页面/组件的 stories + 测试，再逐步扩展覆盖范围；即便中途暂停，也不会影响现有功能与测试体系。
- 主要风险可前置消化：兼容性（React 19 / Vite 7 / Storybook 版本矩阵）、Vitest 集成模式选择，以及 mock 数据一致性问题，都已在文档中列为风险项并可通过小范围 PoC、约定 fixtures 规范等方式在早期阶段验证和收敛。

综合上述因素，本方案在当前项目架构与测试体系下**技术上可行、影响范围可控**，适合作为中等粒度的前端测试能力增强工作按阶段推进。
