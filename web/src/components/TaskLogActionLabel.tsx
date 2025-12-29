import { Icon } from "@iconify/react";

const ACTION_LABELS: Record<string, { label: string; icon: string }> = {
	"image-pull": { label: "镜像拉取", icon: "mdi:download" },
	"restart-unit": { label: "重启服务", icon: "mdi:refresh" },
	"start-unit": { label: "启动服务", icon: "mdi:play-circle-outline" },
	"unit-health-check": { label: "健康检查", icon: "mdi:heart-pulse" },
	"image-verify": { label: "镜像核验", icon: "mdi:shield-check-outline" },
};

export function TaskLogActionLabel(props: { action: string }) {
	const { action } = props;
	const info = ACTION_LABELS[action];
	if (!info) {
		return <span className="font-mono">{action}</span>;
	}

	return (
		<span className="flex flex-wrap items-center gap-1">
			<Icon icon={info.icon} className="text-sm opacity-80" />
			<span className="font-sans font-semibold">{info.label}</span>
			<span className="badge badge-ghost badge-xs font-mono">{action}</span>
		</span>
	);
}
