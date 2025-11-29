# Repository Guidelines

## Project Structure & Module Organization
- `src/`: Rust HTTP service entry point and core logic.
- `tests/`: Rust integration/E2E tests plus `tests/mock-bin` fake `podman`/`systemctl`.
- `web/`: Vite/React admin UI (`web/src/**`, Playwright tests in `web/tests/ui`).
- `migrations/`: SQLx SQLite migrations.
- `scripts/`, `systemd/`, `docs/`: helper scripts, systemd units, and design/ops docs.

## Build, Test, and Development Commands
- Backend build: `cargo build --bin pod-upgrade-trigger`.
- Frontend build: `cd web && (bun install || npm install) && bun run build` (or `npm run build`).
- Dev server (recommended): `scripts/dev-server.sh` (builds web bundle if needed and runs `http-server` on `127.0.0.1:25111`).
- Test server: `scripts/test-server.sh` (in-memory DB on `127.0.0.1:25211`).
- Rust tests: `cargo test`; backend E2E suite: `scripts/test-e2e.sh`.
- UI E2E: `scripts/test-ui-e2e.sh` (real backend) and `scripts/test-ui-e2e-mock.sh` (mock-only, Vite preview).

## Coding Style & Naming Conventions
- Rust: use `cargo fmt` defaults; modules and functions in `snake_case`, types in `PascalCase`, constants in `SCREAMING_SNAKE_CASE`.
- Web: use Biome (`npm run lint`, `npm run format` or `bun run …`) and TypeScript strictness; React components in `PascalCase`, hooks in `useCamelCase`.
- Prefer small, focused modules; follow patterns in nearby files instead of introducing new styles.

## Testing Guidelines
- Place new Rust tests under `tests/`; keep names descriptive and cover error paths.
- Place new UI tests under `web/tests/ui` using Playwright + TypeScript.
- Before opening a PR, run at least: `cargo test` and either `scripts/test-e2e.sh` or the relevant UI E2E script.

## Commit & Pull Request Guidelines
- Use conventional commits, e.g. `feat:`, `feat(web):`, `fix:`, `docs:`, `test:`, `chore:`.
- Keep each commit focused; include tests/docs when behavior changes.
- PRs should describe the problem, approach, and testing done; link related issues and attach screenshots for UI changes when applicable.

## Configuration, Security, and Agent-Specific Notes
- See `README.md` and `docs/*.md` for `PODUP_*` environment variables, state directory layout, and deployment guidance.
- Do not commit secrets, local `PODUP_STATE_DIR` contents, `target/`, or `web/node_modules`.
- Automation or AI agents should prefer the scripts in `scripts/` for starting servers and running tests, and avoid modifying generated artifacts in `target/`, `web/dist`, or `web/node_modules`.
- For DaisyUI-related UI or theme work done via ChatGPT/LLMs, follow the official “ChatGPT setup for daisyUI”: enable web/search tools and prefix prompts with `https://daisyui.com/llms.txt` so the model can read DaisyUI’s compact docs before generating code.
