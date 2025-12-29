import type { JSX } from "react";

export type ManualServiceUpdate = {
	status: "tag_update_available" | "latest_ahead" | "up_to_date" | "unknown";
	tag?: string;
	running_digest?: string;
	remote_tag_digest?: string;
	remote_latest_digest?: string;
	checked_at?: number;
	stale?: boolean;
	reason?: string;
};

export function ManualUpdateBadge({
	update,
}: {
	update?: ManualServiceUpdate | null;
}): JSX.Element | null {
	if (!update) return null;

	const tag = update.tag?.trim() ? update.tag.trim() : null;

	if (update.status === "tag_update_available") {
		return (
			<div className="flex items-center gap-1">
				<span className="badge badge-warning badge-sm">
					{tag ? `有新版本 ${tag}` : "有新版本"}
				</span>
			</div>
		);
	}
	if (update.status === "latest_ahead") {
		return (
			<div className="flex items-center gap-1">
				<span className="badge badge-info badge-sm">有更高版本 latest</span>
			</div>
		);
	}
	if (update.status === "up_to_date") {
		return (
			<div className="flex items-center gap-1">
				<span className="badge badge-success badge-sm">已是最新</span>
			</div>
		);
	}

	return (
		<div className="flex items-center gap-1">
			<div className="tooltip" data-tip={update.reason || "未知原因"}>
				<span className="badge badge-ghost badge-sm border-base-content/20 text-base-content/50">
					未知
				</span>
			</div>
		</div>
	);
}
