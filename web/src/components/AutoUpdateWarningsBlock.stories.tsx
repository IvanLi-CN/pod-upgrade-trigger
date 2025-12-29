import type { Meta, StoryObj } from "@storybook/react";
import { makeAutoUpdateWarningsProps } from "../testing/fixtures";
import { AutoUpdateWarningsBlock } from "./AutoUpdateWarningsBlock";

const meta: Meta<typeof AutoUpdateWarningsBlock> = {
	title: "Components/AutoUpdateWarningsBlock",
	component: AutoUpdateWarningsBlock,
	tags: ["autodocs"],
};

export default meta;
type Story = StoryObj<typeof AutoUpdateWarningsBlock>;

const warningOnly = makeAutoUpdateWarningsProps({ includeError: false });
const mixed = makeAutoUpdateWarningsProps({ includeError: true });

export const WarningOnly: Story = {
	name: "Warning only",
	args: {
		summary: warningOnly.summary,
		details: warningOnly.details,
	},
};

export const MixedWithErrors: Story = {
	name: "Mixed with errors",
	args: {
		summary: mixed.summary,
		details: mixed.details,
	},
};
