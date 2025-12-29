import { Icon } from "@iconify/react";
import { useMemo, useState } from "react";
import { useToast } from "./Toast";

type TaskLogMetaHints = {
	unit?: string;
	image?: string | null;
	result_status?: string;
	result_message?: string;
};

type ImageVerifyPlatform = {
	os: string;
	arch: string;
	variant?: string | null;
};

type ImageVerifyMeta = {
	platform?: ImageVerifyPlatform | null;
	remote_index_digest?: string;
	remote_platform_digest?: string;
	pulled_digest?: string;
	running_digest?: string;
	remote_error?: string | null;
	local_error?: string | null;
};

function parseMetaObject(meta: unknown): Record<string, unknown> | null {
	if (!meta) return null;

	if (typeof meta === "string") {
		try {
			const parsed = JSON.parse(meta) as unknown;
			return parseMetaObject(parsed);
		} catch {
			return null;
		}
	}

	if (typeof meta !== "object") return null;
	return meta as Record<string, unknown>;
}

function readNonEmptyString(
	obj: Record<string, unknown>,
	key: string,
): string | null {
	const value = obj[key];
	if (typeof value !== "string") return null;
	const trimmed = value.trim();
	return trimmed.length > 0 ? trimmed : null;
}

function readStringOrNull(
	obj: Record<string, unknown>,
	key: string,
): string | null | undefined {
	const value = obj[key];
	if (value === undefined) return undefined;
	if (value === null) return null;
	if (typeof value !== "string") return undefined;
	const trimmed = value.trim();
	return trimmed.length > 0 ? trimmed : null;
}

function parsePlatform(value: unknown): ImageVerifyPlatform | null {
	if (!value) return null;
	if (typeof value !== "object") return null;
	const candidate = value as { [key: string]: unknown };
	const os = typeof candidate.os === "string" ? candidate.os.trim() : "";
	const arch = typeof candidate.arch === "string" ? candidate.arch.trim() : "";
	const variant =
		candidate.variant === null
			? null
			: typeof candidate.variant === "string"
				? candidate.variant.trim() || null
				: null;
	if (!os || !arch) return null;
	return { os, arch, variant };
}

function formatPlatform(platform: ImageVerifyPlatform): string {
	if (platform.variant)
		return `${platform.os}/${platform.arch}/${platform.variant}`;
	return `${platform.os}/${platform.arch}`;
}

function extractImageVerifyMeta(meta: unknown): ImageVerifyMeta | null {
	const obj = parseMetaObject(meta);
	if (!obj) return null;

	const hasAny =
		"remote_platform_digest" in obj ||
		"remote_index_digest" in obj ||
		"pulled_digest" in obj ||
		"running_digest" in obj ||
		"remote_error" in obj ||
		"local_error" in obj ||
		"platform" in obj;

	if (!hasAny) return null;

	const platform = parsePlatform(obj.platform);
	const remote_platform_digest =
		readNonEmptyString(obj, "remote_platform_digest") ?? undefined;
	const remote_index_digest =
		readNonEmptyString(obj, "remote_index_digest") ?? undefined;
	const pulled_digest = readNonEmptyString(obj, "pulled_digest") ?? undefined;
	const running_digest = readNonEmptyString(obj, "running_digest") ?? undefined;
	const remote_error = readStringOrNull(obj, "remote_error");
	const local_error = readStringOrNull(obj, "local_error");

	return {
		platform,
		remote_platform_digest,
		remote_index_digest,
		pulled_digest,
		running_digest,
		remote_error: remote_error ?? undefined,
		local_error: local_error ?? undefined,
	};
}

function extractMetaHints(meta: unknown): TaskLogMetaHints | null {
	const obj = parseMetaObject(meta);
	if (!obj) return null;

	const unit = readNonEmptyString(obj, "unit") ?? undefined;
	const imageRaw = obj.image;
	const image =
		imageRaw === null
			? null
			: typeof imageRaw === "string"
				? imageRaw.trim() || null
				: null;
	const result_status = readNonEmptyString(obj, "result_status") ?? undefined;
	const result_message = readNonEmptyString(obj, "result_message") ?? undefined;

	if (!unit && !image && !result_status && !result_message) return null;
	return { unit, image, result_status, result_message };
}

function isLongMessage(message: string): boolean {
	const lineCount = message.split("\n").length;
	return lineCount > 3 || message.length > 200;
}

function buildCollapsedPreview(message: string): string {
	const lines = message.split("\n");
	if (lines.length > 3) {
		return `${lines.slice(0, 3).join("\n")}\n…`;
	}
	if (message.length > 200) {
		return `${message.slice(0, 200)}…`;
	}
	return message;
}

export function TaskLogMetaDetails(props: {
	meta: unknown;
	unitAlreadyShown?: boolean;
}) {
	const { meta, unitAlreadyShown } = props;
	const { pushToast } = useToast();
	const hints = useMemo(() => extractMetaHints(meta), [meta]);
	const imageVerify = useMemo(() => extractImageVerifyMeta(meta), [meta]);
	const message = hints?.result_message ?? null;
	const long = message ? isLongMessage(message) : false;
	const [expanded, setExpanded] = useState(() => !long);

	const hintEntries = useMemo(() => {
		if (!hints) return [];
		const entries: Array<{ key: string; value: string }> = [];

		if (!unitAlreadyShown && hints.unit) {
			entries.push({ key: "unit", value: hints.unit });
		}
		if (hints.image) {
			entries.push({ key: "image", value: hints.image });
		}
		if (hints.result_status) {
			entries.push({ key: "result_status", value: hints.result_status });
		}
		return entries;
	}, [hints, unitAlreadyShown]);

	const handleCopy = async (label: string, value: string) => {
		try {
			await navigator.clipboard.writeText(value);
			pushToast({
				variant: "success",
				title: "已复制",
				message: `${label}: ${value}`,
			});
		} catch {
			pushToast({
				variant: "warning",
				title: "复制失败",
				message: "浏览器未允许访问剪贴板。",
			});
		}
	};

	const remoteDigest =
		imageVerify?.remote_platform_digest ??
		imageVerify?.remote_index_digest ??
		null;
	const pulledDigest = imageVerify?.pulled_digest ?? null;
	const runningDigest = imageVerify?.running_digest ?? null;
	const remoteError = imageVerify?.remote_error ?? null;
	const localError = imageVerify?.local_error ?? null;

	const matchRemotePulled =
		remoteDigest && pulledDigest ? remoteDigest === pulledDigest : null;
	const matchPulledRunning =
		pulledDigest && runningDigest ? pulledDigest === runningDigest : null;
	const overallMatch =
		remoteError || !remoteDigest || !pulledDigest || !runningDigest
			? null
			: remoteDigest === pulledDigest && pulledDigest === runningDigest;

	const showImageVerifyBlock = Boolean(imageVerify);

	if (!showImageVerifyBlock && !message && hintEntries.length === 0)
		return null;

	return (
		<div className="mt-0.5 space-y-0.5">
			{showImageVerifyBlock ? (
				<div
					className={`rounded border px-2 py-1 ${
						overallMatch === true
							? "border-success/40 bg-success/5"
							: overallMatch === false
								? "border-error/40 bg-error/5"
								: "border-base-200 bg-base-200/30"
					}`}
				>
					<div className="mb-1 flex flex-wrap items-center justify-between gap-2">
						<div className="flex items-center gap-1 text-[11px] font-semibold text-base-content/80">
							<Icon icon="mdi:shield-check-outline" className="text-base" />
							<span>镜像核验</span>
							{imageVerify?.platform ? (
								<span className="badge badge-ghost badge-xs font-mono">
									{formatPlatform(imageVerify.platform)}
								</span>
							) : null}
						</div>
						<span
							className={`badge badge-xs ${
								overallMatch === true
									? "badge-success"
									: overallMatch === false
										? "badge-error"
										: "badge-warning"
							}`}
						>
							{overallMatch === true
								? "match"
								: overallMatch === false
									? "mismatch"
									: "unknown"}
						</span>
					</div>

					<div className="grid grid-cols-[auto,1fr,auto] items-start gap-x-2 gap-y-1 text-[11px]">
						<span className="pt-0.5 text-base-content/60">Remote</span>
						<code
							className={`break-all rounded bg-base-200/60 px-1 font-mono text-[10px] ${
								remoteError ? "text-warning" : "text-base-content/80"
							}`}
							title={remoteDigest ?? undefined}
						>
							{remoteDigest ??
								(remoteError
									? "remote digest unavailable"
									: "remote digest missing")}
						</code>
						{remoteDigest ? (
							<button
								type="button"
								className="btn btn-ghost btn-xs h-auto min-h-0 px-1 py-0"
								onClick={() => handleCopy("remote", remoteDigest)}
								aria-label="Copy remote digest"
								title="复制 remote digest"
							>
								<Icon icon="mdi:content-copy" className="text-base" />
							</button>
						) : (
							<span />
						)}

						<span className="pt-0.5 text-base-content/60">Pulled</span>
						<div className="space-y-0.5">
							<code
								className="block break-all rounded bg-base-200/60 px-1 font-mono text-[10px] text-base-content/80"
								title={pulledDigest ?? undefined}
							>
								{pulledDigest ?? "—"}
							</code>
							{matchRemotePulled !== null ? (
								<span
									className={`badge badge-xs ${
										matchRemotePulled ? "badge-success" : "badge-error"
									}`}
								>
									{matchRemotePulled ? "matches remote" : "differs from remote"}
								</span>
							) : null}
						</div>
						{pulledDigest ? (
							<button
								type="button"
								className="btn btn-ghost btn-xs h-auto min-h-0 px-1 py-0"
								onClick={() => handleCopy("pulled", pulledDigest)}
								aria-label="Copy pulled digest"
								title="复制 pulled digest"
							>
								<Icon icon="mdi:content-copy" className="text-base" />
							</button>
						) : (
							<span />
						)}

						<span className="pt-0.5 text-base-content/60">Running</span>
						<div className="space-y-0.5">
							<code
								className="block break-all rounded bg-base-200/60 px-1 font-mono text-[10px] text-base-content/80"
								title={runningDigest ?? undefined}
							>
								{runningDigest ?? "—"}
							</code>
							{matchPulledRunning !== null ? (
								<span
									className={`badge badge-xs ${
										matchPulledRunning ? "badge-success" : "badge-error"
									}`}
								>
									{matchPulledRunning
										? "matches pulled"
										: "differs from pulled"}
								</span>
							) : null}
						</div>
						{runningDigest ? (
							<button
								type="button"
								className="btn btn-ghost btn-xs h-auto min-h-0 px-1 py-0"
								onClick={() => handleCopy("running", runningDigest)}
								aria-label="Copy running digest"
								title="复制 running digest"
							>
								<Icon icon="mdi:content-copy" className="text-base" />
							</button>
						) : (
							<span />
						)}
					</div>

					{remoteError ? (
						<div className="mt-1 rounded border border-warning/40 bg-warning/10 px-2 py-1 text-[11px]">
							<span className="font-semibold text-warning">Remote error</span>
							<span className="ml-2 break-words text-base-content/70">
								{remoteError}
							</span>
						</div>
					) : null}
					{localError ? (
						<div className="mt-1 rounded border border-error/40 bg-error/10 px-2 py-1 text-[11px]">
							<span className="font-semibold text-error">Local error</span>
							<span className="ml-2 break-words text-base-content/70">
								{localError}
							</span>
						</div>
					) : null}
				</div>
			) : null}

			{message ? (
				<div className="rounded border border-base-200 bg-base-200/40 px-2 py-1">
					<div className="whitespace-pre-wrap break-words text-[11px] text-base-content/80">
						{long && !expanded ? buildCollapsedPreview(message) : message}
					</div>
					{long ? (
						<button
							type="button"
							className="btn btn-ghost btn-xs mt-1 h-auto min-h-0 px-1 py-0 text-[11px]"
							onClick={() => setExpanded((prev) => !prev)}
						>
							{expanded ? "收起详情" : "展开详情"}
						</button>
					) : null}
				</div>
			) : null}
			{hintEntries.length > 0 ? (
				<div className="flex flex-wrap gap-x-2 gap-y-0.5 text-[10px] text-base-content/60">
					{hintEntries.map((entry) => (
						<span key={entry.key}>
							{entry.key} · {entry.value}
						</span>
					))}
				</div>
			) : null}
		</div>
	);
}
