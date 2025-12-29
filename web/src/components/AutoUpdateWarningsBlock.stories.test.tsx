import { composeStories } from "@storybook/react";
import { expect, screen } from "@storybook/test";
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, it } from "vitest";
import * as AutoUpdateStories from "./AutoUpdateWarningsBlock.stories";

const { WarningOnly, MixedWithErrors } = composeStories(AutoUpdateStories);

afterEach(() => cleanup());

describe("AutoUpdateWarningsBlock stories", () => {
	it("renders the warning-only summary and detail", async () => {
		render(<WarningOnly />);

		await screen.findByText(/Auto-update warnings/i);
		expect(
			screen.getByText("Last auto-update completed with warnings"),
		).toBeInTheDocument();
		expect(screen.getByText(/simulated dry-run warning/i)).toBeInTheDocument();
	});

	it("renders mixed warnings with error details", async () => {
		render(<MixedWithErrors />);

		await screen.findByText(/Auto-update warnings \(2\)/i);
		expect(
			screen.getByText(/auto-update completed with warnings and 1 error/i),
		).toBeInTheDocument();
		expect(screen.getByText(/simulated fatal warning/i)).toBeInTheDocument();
	});
});
