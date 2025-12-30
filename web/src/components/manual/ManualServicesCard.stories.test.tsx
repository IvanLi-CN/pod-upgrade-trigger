import { composeStories } from "@storybook/react";
import { expect, screen } from "@storybook/test";
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, it } from "vitest";
import * as ManualServicesCardStories from "./ManualServicesCard.stories";

const { Empty, Mixed, Refreshing, Loading } = composeStories(
	ManualServicesCardStories,
);

afterEach(() => cleanup());

describe("ManualServicesCard stories", () => {
	it("renders empty message when no services", async () => {
		render(<Empty />);
		expect(await screen.findByText("暂无可升级的服务。")).toBeInTheDocument();
	});

	it("renders mixed update badge states", async () => {
		render(<Mixed />);
		expect(await screen.findByText(/有新版本\s*v1\.0\.1/)).toBeInTheDocument();
		expect(await screen.findByText(/有更高版本\s*latest/)).toBeInTheDocument();
		expect(await screen.findByText("已是最新")).toBeInTheDocument();
		expect(await screen.findByText("未知")).toBeInTheDocument();
	});

	it("disables refresh button and shows animate-spin while refreshing", async () => {
		render(<Refreshing />);
		const button = await screen.findByRole("button", { name: "刷新更新状态" });
		expect(button).toBeDisabled();
		expect(button.querySelector(".animate-spin")).not.toBeNull();
	});

	it("renders loading state when services are loading", async () => {
		render(<Loading />);
		expect(await screen.findByText("正在加载服务列表…")).toBeInTheDocument();
		expect(screen.queryByText("暂无可升级的服务。")).toBeNull();
	});
});
