import { Icon } from "@iconify/react";
import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useApi } from "../hooks/useApi";

type EventSummary = {
	id: number;
	request_id: string;
	ts: number;
	method: string;
	path: string | null;
	status: number;
	action: string;
};

type EventsResponse = {
	events: EventSummary[];
};

type TimelineEntry = EventSummary;

type RateLimitHint = {
	title: string;
	level: "ok" | "warn";
	description: string;
};

export default function DashboardPage() {
	const { status, getJson } = useApi();
	const [events, setEvents] = useState<TimelineEntry[]>([]);
	const navigate = useNavigate();

	useEffect(() => {
		let cancelled = false;
		(async () => {
			try {
				const data = await getJson<EventsResponse>("/api/events?limit=20");
				if (!cancelled && Array.isArray(data.events)) {
					setEvents(
						data.events.map((e) => ({
							...e,
							path: e.path ?? null,
						})),
					);
				}
			} catch {
				// ignore dashboard errors, surface in Events view instead
			}
		})();

		return () => {
			cancelled = true;
		};
	}, [getJson]);

	const lastScheduler = useMemo(
		() => events.find((e) => e.action === "scheduler"),
		[events],
	);

	const lastManual = useMemo(
		() => events.find((e) => e.action === "manual-trigger"),
		[events],
	);

	const lastGithub = useMemo(
		() => events.find((e) => e.action === "github-webhook"),
		[events],
	);

	const rateHint: RateLimitHint = useMemo(() => {
		const recentManuals = events.filter((e) => e.action === "manual-trigger");
		if (!recentManuals.length) {
			return {
				title: "Rate limit relaxed",
				level: "ok",
				description:
					"No recent manual triggers; limits are unlikely to be hit.",
			};
		}

		return {
			title: "Rate limit activity",
			level: "warn",
			description:
				"Recent manual triggers observed. Use the Maintenance page to prune state if rate limits are hit.",
		};
	}, [events]);

	const openTimelineItem = (event: TimelineEntry) => {
		navigate(`/events?request_id=${encodeURIComponent(event.request_id)}`);
	};

	const formatTs = (ts: number | null | undefined) => {
		if (!ts || ts <= 0) return "--";
		return new Date(ts * 1000).toLocaleString();
	};

	return (
		<div className="space-y-6">
			<section className="grid gap-4 md:grid-cols-3">
				<div className="card bg-base-100 shadow-sm">
					<div className="card-body gap-3">
						<div className="flex items-center justify-between">
							<span className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
								Health
							</span>
							<Icon icon="mdi:heart-pulse" className="text-xl text-primary" />
						</div>
						<p className="text-2xl font-semibold">
							{status.health === "ok"
								? "Service healthy"
								: status.health === "error"
									? "Degraded"
									: "Probing…"}
						</p>
						<p className="text-xs text-base-content/70">
							/health · /sse/hello · scheduler interval{" "}
							{status.scheduler.intervalSecs}s
						</p>
					</div>
				</div>
				<div className="card bg-base-100 shadow-sm">
					<div className="card-body gap-3">
						<div className="flex items-center justify-between">
							<span className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
								Scheduler
							</span>
							<Icon icon="mdi:autorenew" className="text-xl text-secondary" />
						</div>
						<p className="text-2xl font-semibold">
							tick #
							{status.scheduler.lastIteration ?? lastScheduler?.id ?? "--"}
						</p>
						<p className="text-xs text-base-content/70">
							last event · {formatTs(lastScheduler?.ts ?? null)}
						</p>
					</div>
				</div>
				<div className="card bg-base-100 shadow-sm">
					<div className="card-body gap-3">
						<div className="flex items-center justify-between">
							<span className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
								Activity
							</span>
							<Icon icon="mdi:history" className="text-xl text-accent" />
						</div>
						<div className="space-y-1 text-xs text-base-content/80">
							<div>
								<span className="font-semibold">Manual</span> ·{" "}
								{formatTs(lastManual?.ts ?? null)}
							</div>
							<div>
								<span className="font-semibold">GitHub</span> ·{" "}
								{formatTs(lastGithub?.ts ?? null)}
							</div>
						</div>
						<Link to="/events" className="btn btn-xs btn-outline self-start">
							查看事件
						</Link>
					</div>
				</div>
			</section>

			<section className="grid gap-4 md:grid-cols-[2fr_1fr]">
				<div className="card bg-base-100 shadow-sm">
					<div className="card-body gap-3">
						<div className="flex items-center justify-between">
							<h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
								最近事件
							</h2>
							<Link to="/events" className="btn btn-ghost btn-xs gap-1">
								<Icon icon="mdi:open-in-new" />
								全部
							</Link>
						</div>
						<div className="space-y-2">
							{events.length === 0 && (
								<p className="text-xs text-base-content/60">暂无事件记录。</p>
							)}
							{events.map((event) => (
								<button
									type="button"
									key={event.id}
									className="flex w-full items-center justify-between rounded-lg border border-base-200 bg-base-100 px-3 py-2 text-left text-xs hover:border-primary/60 hover:bg-base-200"
									onClick={() => openTimelineItem(event)}
								>
									<div className="flex min-w-0 flex-1 flex-col">
										<div className="flex items-center gap-2">
											<span className="badge badge-ghost badge-xs">
												{event.action}
											</span>
											<span className="truncate text-[11px] text-base-content/70">
												{event.method} {event.path ?? "-"}
											</span>
										</div>
										<span className="mt-0.5 text-[10px] text-base-content/60">
											{formatTs(event.ts)} · req {event.request_id}
										</span>
									</div>
									<span
										className={`badge badge-xs ${
											event.status >= 500
												? "badge-error"
												: event.status >= 400
													? "badge-warning"
													: "badge-success"
										}`}
									>
										{event.status}
									</span>
								</button>
							))}
						</div>
					</div>
				</div>

				<div className="card bg-base-100 shadow-sm">
					<div className="card-body gap-3">
						<div className="flex items-center justify-between">
							<h2 className="text-lg font-semibold uppercase tracking-wide text-base-content/70">
								速率限制
							</h2>
							<Link
								to="/maintenance#ratelimit"
								className="btn btn-ghost btn-xs gap-1"
							>
								<Icon icon="mdi:tune" />
								维护
							</Link>
						</div>
						<div className="flex items-start gap-2 text-xs">
							<Icon
								icon={
									rateHint.level === "ok"
										? "mdi:check-circle"
										: "mdi:alert-circle"
								}
								className={
									rateHint.level === "ok" ? "text-success" : "text-warning"
								}
							/>
							<div className="space-y-1">
								<div className="font-semibold text-base-content">
									{rateHint.title}
								</div>
								<p className="text-base-content/70">{rateHint.description}</p>
							</div>
						</div>
						<p className="text-[10px] text-base-content/50">
							实际配额来源于 SQLite 中的 rate_limit_tokens
							表；当前界面基于事件日志进行近似估算。
						</p>
					</div>
				</div>
			</section>
		</div>
	);
}
