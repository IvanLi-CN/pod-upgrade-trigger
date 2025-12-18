import { Icon } from "@iconify/react";
import {
	BrowserRouter,
	Link,
	Navigate,
	Route,
	Routes,
	useLocation,
	useNavigate,
} from "react-router-dom";
import DashboardPage from "./pages/DashboardPage";
import EventsPage from "./pages/EventsPage";
import ManualPage from "./pages/ManualPage";
import MaintenancePage from "./pages/MaintenancePage";
import SettingsPage from "./pages/SettingsPage";
import WebhooksPage from "./pages/WebhooksPage";
import TasksPage from "./pages/TasksPage";
import UnauthorizedPage from "./pages/UnauthorizedPage";
import { ApiProvider, useApi } from "./hooks/useApi";
import { ToastProvider, ToastViewport, useToast } from "./components/Toast";
import MockConsole from "./mocks/MockConsole";
import { useVersionCheck } from "./hooks/useVersionCheck";

function ensureLeadingV(value: string | null | undefined): string | null {
	if (!value) return null;
	const trimmed = value.trim();
	if (!trimmed) return null;
	return trimmed.startsWith("v") ? trimmed : `v${trimmed}`;
}

function formatCurrentVersionLabel(input: {
	releaseTag: string | null;
	packageVersion: string | null;
}): string {
	const tag = ensureLeadingV(input.releaseTag);
	if (tag) return tag;
	const pkg = ensureLeadingV(input.packageVersion);
	if (pkg) return pkg;
	return "v--";
}

export function TopStatusBar() {
	const { status, postJson } = useApi();
	const { health, scheduler, sseStatus, now } = status;
	const navigate = useNavigate();
	const { pushToast } = useToast();
	const version = useVersionCheck();

	const latestTag = version.latest?.releaseTag;
	const showNewVersion = version.hasUpdate === true && latestTag;

	const currentVersionLabel = formatCurrentVersionLabel({
		releaseTag: status.version.releaseTag,
		packageVersion: status.version.package,
	});

	const handleSelfUpdate = async () => {
		if (!latestTag) return;

		const ok = window.confirm(
			[
				"将触发后端自更新（self-update）。",
				"服务可能短暂重启，页面可能在短时间内不可用或刷新失败。",
				"是否继续？（是否 dry-run 由服务端环境变量决定）",
			].join("\n"),
		);
		if (!ok) return;

		type SelfUpdateResponse = { task_id?: string | null; dry_run?: boolean | null };
		try {
			const data = await postJson<SelfUpdateResponse>("/api/self-update/run", {});
			const taskId = data.task_id ? String(data.task_id) : "";
			if (!taskId) {
				pushToast({
					variant: "error",
					title: "更新已触发，但未返回 task_id",
					message: "请稍后在 Tasks 页面确认任务状态。",
				});
				navigate("/tasks");
				return;
			}

			pushToast({
				variant: "success",
				title: data.dry_run ? "已触发更新（dry-run）" : "已触发更新任务",
				message: `task_id=${taskId}${data.dry_run ? " · dry-run，仅验证下载/校验" : ""}`,
			});
			navigate(`/tasks?task_id=${encodeURIComponent(taskId)}`);
		} catch (err) {
			const statusCode =
				err && typeof err === "object" && "status" in err ? String(err.status) : "";
			const message =
				err && typeof err === "object" && "message" in err && err.message
					? String(err.message)
					: "触发更新失败";
			pushToast({
				variant: "error",
				title: "触发更新失败",
				message: statusCode ? `${statusCode} · ${message}` : message,
			});
		}
	};

	return (
		<header className="navbar sticky top-0 z-20 border-b border-base-300 bg-base-100/90 backdrop-blur">
			<div className="navbar-start gap-2 px-4">
				<span className="flex items-center gap-2 text-lg font-title font-semibold">
					<Icon icon="mdi:cat" className="text-2xl text-primary" />
					Pod Upgrade Trigger
				</span>
				<span className="badge badge-sm badge-outline">
					{currentVersionLabel}
				</span>
				{showNewVersion ? (
					<div className="dropdown">
						<button
							type="button"
							className="badge badge-warning badge-sm gap-1"
							tabIndex={0}
							aria-label={`新版本菜单 ${latestTag}`}
						>
							<Icon
								icon="mdi:arrow-up-bold-circle-outline"
								className="text-base"
							/>
							{latestTag}
						</button>
						<ul
							className="dropdown-content menu menu-sm z-[60] mt-2 w-56 rounded-box border border-base-300 bg-base-100 p-2 shadow"
						>
							<li>
								<button type="button" onClick={handleSelfUpdate}>
									立即更新
								</button>
							</li>
							<li>
								<a
									href={`https://github.com/ivanli-cn/pod-upgrade-trigger/tree/${encodeURIComponent(latestTag)}`}
									target="_blank"
									rel="noreferrer"
								>
									跳转到该版本代码页
								</a>
							</li>
						</ul>
					</div>
				) : null}
				<span className="badge badge-sm badge-outline hidden sm:inline-flex">
					{health === "ok"
						? "Healthy"
						: health === "error"
							? "Degraded"
							: "Checking…"}
				</span>
			</div>
			<div className="navbar-center hidden md:flex">
				<div className="join">
					<span className="join-item badge badge-ghost gap-1">
						<Icon icon="mdi:timer-sand" className="text-lg" />
						{scheduler.intervalSecs}s
					</span>
					<span className="join-item badge badge-ghost gap-1">
						<Icon icon="mdi:autorenew" className="text-lg" />
						tick #{scheduler.lastIteration ?? "-"}
					</span>
					<span className="join-item badge badge-ghost gap-1">
						<Icon icon="mdi:access-point" className="text-lg" />
						{sseStatus === "open"
							? "SSE ok"
							: sseStatus === "error"
								? "SSE error"
								: "SSE…"}
					</span>
				</div>
			</div>
			<div className="navbar-end gap-2 px-4">
				<span className="hidden text-base text-base-content/70 sm:inline">
					{now.toLocaleTimeString()}
				</span>
			</div>
		</header>
	);
}

function SideNav() {
	const location = useLocation();
	const entries = [
		{ to: "/", label: "Dashboard", icon: "mdi:view-dashboard" },
		{ to: "/services", label: "Services", icon: "mdi:play-circle-outline" },
		{ to: "/webhooks", label: "Webhooks", icon: "mdi:webhook" },
		{ to: "/tasks", label: "Tasks", icon: "mdi:clipboard-text-clock-outline" },
		{
			to: "/events",
			label: "Events",
			icon: "mdi:file-document-multiple-outline",
		},
		{ to: "/maintenance", label: "Maintenance", icon: "mdi:toolbox-outline" },
		{ to: "/settings", label: "Settings", icon: "mdi:cog-outline" },
	];

	return (
		<aside className="h-full w-56 border-r border-base-300 bg-base-100/80 backdrop-blur">
			<nav className="flex h-full flex-col gap-2 p-3">
				<ul className="menu menu-sm flex-1 gap-1">
					{entries.map((entry) => {
						const active =
							entry.to === "/"
								? location.pathname === "/"
								: location.pathname.startsWith(entry.to);
						return (
							<li key={entry.to}>
								<Link
									to={entry.to}
									className={active ? "active font-semibold" : undefined}
									aria-current={active ? "page" : undefined}
								>
									<Icon icon={entry.icon} className="text-lg" />
									<span>{entry.label}</span>
								</Link>
							</li>
						);
					})}
				</ul>
				<div className="mt-auto flex flex-col gap-1 text-[11px] text-base-content/60">
					<span>Webhook auto-update UI</span>
				</div>
			</nav>
		</aside>
	);
}

function Layout() {
	return (
		<div className="flex min-h-screen flex-col bg-base-200 text-base-content">
			<TopStatusBar />
			<div className="flex min-h-0 flex-1">
				<SideNav />
				<main className="flex-1 overflow-y-auto">
					<div className="mx-auto flex max-w-6xl flex-col gap-6 px-4 py-6">
						<Routes>
							<Route path="/" element={<DashboardPage />} />
							<Route path="/services" element={<ManualPage />} />
							<Route path="/manual" element={<ManualRedirect />} />
							<Route path="/webhooks" element={<WebhooksPage />} />
							<Route path="/tasks" element={<TasksPage />} />
							<Route path="/events" element={<EventsPage />} />
							<Route path="/maintenance" element={<MaintenancePage />} />
							<Route path="/settings" element={<SettingsPage />} />
							<Route path="/401" element={<UnauthorizedPage />} />
							<Route path="*" element={<NotFoundFallback />} />
						</Routes>
					</div>
				</main>
			</div>
			<ToastViewport />
		</div>
	);
}

function NotFoundFallback() {
	const navigate = useNavigate();
	return (
		<div className="hero min-h-[60vh]">
			<div className="hero-content text-center">
				<div className="max-w-md space-y-4">
					<h1 className="text-3xl font-bold">404 · 页面不存在</h1>
					<p className="text-lg text-base-content/70">
						所请求的路由不存在，可能是链接已失效或路径输入有误。
					</p>
					<button
						type="button"
						className="btn btn-primary btn-sm"
						onClick={() => navigate("/")}
					>
						返回 Dashboard
					</button>
				</div>
			</div>
		</div>
	);
}

function ManualRedirect() {
	const location = useLocation();
	return (
		<Navigate to={`/services${location.search}${location.hash}`} replace />
	);
}

type AppProps = {
	mockEnabled?: boolean;
};

export default function App({ mockEnabled = false }: AppProps) {
	return (
		<BrowserRouter>
			<ToastProvider>
				<ApiProvider>
					<Layout />
				</ApiProvider>
			</ToastProvider>
			{mockEnabled ? <MockConsole /> : null}
		</BrowserRouter>
	);
}
