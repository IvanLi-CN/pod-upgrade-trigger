import { Icon } from "@iconify/react";
import type { CSSProperties, ReactNode } from "react";

type SectionProps = {
	title: string;
	children: ReactNode;
};

function Section({ title, children }: SectionProps) {
	return (
		<section className="card bg-base-100 shadow-xs">
			<div className="card-body space-y-4">
				<div className="flex items-center gap-2 text-sm font-semibold uppercase tracking-wide text-base-content/70">
					<span>{title}</span>
				</div>
				{children}
			</div>
		</section>
	);
}

export default function DaisyLabPage() {
	return (
		<div className="space-y-6">
			<div className="flex items-center justify-between gap-2">
				<div>
					<h1 className="text-2xl font-bold">DaisyUI Style Lab</h1>
					<p className="text-sm text-base-content/70">
						Visual sweep for major DaisyUI components.
					</p>
				</div>
				<span className="badge badge-outline badge-sm">static preview</span>
			</div>

			<Section title="Buttons & Badges">
				<div className="flex flex-wrap items-center gap-2">
					<button className="btn btn-primary btn-sm" type="button">
						Primary
					</button>
					<button className="btn btn-secondary btn-sm" type="button">
						Secondary
					</button>
					<button className="btn btn-accent btn-sm" type="button">
						Accent
					</button>
					<button className="btn btn-outline btn-sm" type="button">
						Outline
					</button>
					<button className="btn btn-ghost btn-sm" type="button">
						Ghost
					</button>
					<button className="btn btn-link btn-sm" type="button">
						Link
					</button>
					<button className="btn btn-disabled btn-sm" type="button" disabled>
						Disabled
					</button>
					<span className="badge badge-primary">primary</span>
					<span className="badge badge-secondary">secondary</span>
					<span className="badge badge-accent">accent</span>
					<span className="badge badge-ghost">ghost</span>
					<span className="badge badge-outline">outline</span>
				</div>
			</Section>

			<Section title="Inputs & Controls">
				<div className="grid gap-4 md:grid-cols-2">
					<label className="form-control w-full max-w-xs">
						<div className="label">
							<span className="label-text">Text input</span>
						</div>
						<input
							type="text"
							placeholder="Type here"
							className="input input-bordered w-full"
						/>
					</label>

					<label className="form-control w-full max-w-xs">
						<div className="label">
							<span className="label-text">Select</span>
						</div>
						<select className="select select-bordered w-full">
							<option>Pick one</option>
							<option>Option A</option>
							<option>Option B</option>
							<option disabled>Disabled</option>
						</select>
					</label>

					<div className="flex flex-wrap items-center gap-4">
						<label className="label cursor-pointer gap-2">
							<span className="label-text">Checkbox</span>
							<input
								type="checkbox"
								className="checkbox checkbox-primary"
								defaultChecked
							/>
						</label>
						<label className="label cursor-pointer gap-2">
							<span className="label-text">Toggle</span>
							<input
								type="checkbox"
								className="toggle toggle-secondary"
								defaultChecked
							/>
						</label>
						<label className="label cursor-pointer gap-2">
							<span className="label-text">Radio</span>
							<input
								type="radio"
								name="radio-demo"
								className="radio radio-accent"
								defaultChecked
							/>
						</label>
					</div>

					<div className="flex flex-wrap items-center gap-4">
						<div className="form-control w-40">
							<label className="label" htmlFor="range-demo">
								<span className="label-text">Range</span>
							</label>
							<input
								id="range-demo"
								type="range"
								min="0"
								max="100"
								defaultValue="40"
								className="range range-primary"
							/>
						</div>
						<progress
							className="progress progress-accent w-40"
							value="60"
							max="100"
						></progress>
						<div
							className="radial-progress text-secondary"
							style={{ "--value": 75 } as CSSProperties}
						>
							75%
						</div>
					</div>
				</div>
			</Section>

			<Section title="Tabs Variants">
				<div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
					<div>
						<p className="mb-2 text-xs text-base-content/70">Default</p>
						<div className="tabs" role="tablist">
							<button className="tab tab-active" type="button" role="tab">
								Alpha
							</button>
							<button className="tab" type="button" role="tab">
								Beta
							</button>
							<button
								className="tab tab-disabled"
								type="button"
								role="tab"
								disabled
								aria-disabled="true"
							>
								Disabled
							</button>
						</div>
					</div>

					<div>
						<p className="mb-2 text-xs text-base-content/70">Boxed</p>
						<div className="tabs tabs-box" role="tablist">
							<button className="tab tab-active" type="button" role="tab">
								Boxed
							</button>
							<button className="tab" type="button" role="tab">
								Idle
							</button>
							<button className="tab" type="button" role="tab">
								Ghost
							</button>
						</div>
					</div>

					<div>
						<p className="mb-2 text-xs text-base-content/70">Lift</p>
						<div className="tabs tabs-lift" role="tablist">
							<button className="tab" type="button" role="tab">
								One
							</button>
							<button className="tab tab-active" type="button" role="tab">
								Two
							</button>
							<button className="tab" type="button" role="tab">
								Three
							</button>
						</div>
					</div>

					<div>
						<p className="mb-2 text-xs text-base-content/70">
							Border (underline)
						</p>
						<div className="tabs tabs-border" role="tablist">
							<button className="tab tab-active" type="button" role="tab">
								Active
							</button>
							<button className="tab" type="button" role="tab">
								Idle
							</button>
							<button className="tab" type="button" role="tab">
								Hover
							</button>
						</div>
					</div>

					<div>
						<p className="mb-2 text-xs text-base-content/70">
							Tabs Top (with content)
						</p>
						<div className="tabs tabs-top w-full" role="tablist">
							<button
								className="tab tab-active"
								type="button"
								role="tab"
								aria-selected="true"
							>
								First
							</button>
							<button
								className="tab"
								type="button"
								role="tab"
								aria-selected="false"
							>
								Second
							</button>
							<div className="tab-content bg-base-100 p-3">
								Top tab content shows here.
							</div>
						</div>
					</div>

					<div>
						<p className="mb-2 text-xs text-base-content/70">
							Tabs Bottom (with content)
						</p>
						<div className="tabs tabs-bottom w-full" role="tablist">
							<div className="tab-content bg-base-100 p-3">
								Bottom tab content shows here.
							</div>
							<button
								className="tab tab-active"
								type="button"
								role="tab"
								aria-selected="true"
							>
								Summary
							</button>
							<button
								className="tab"
								type="button"
								role="tab"
								aria-selected="false"
							>
								History
							</button>
						</div>
					</div>

					<div className="xl:col-span-3">
						<p className="mb-2 text-xs text-base-content/70">Sizes</p>
						<div className="flex flex-wrap gap-3">
							<div className="tabs tabs-lift tabs-xs" role="tablist">
								<button
									className="tab tab-active"
									type="button"
									role="tab"
									aria-selected="true"
								>
									xs
								</button>
								<button
									className="tab"
									type="button"
									role="tab"
									aria-selected="false"
								>
									tab
								</button>
							</div>
							<div className="tabs tabs-lift tabs-sm" role="tablist">
								<button
									className="tab tab-active"
									type="button"
									role="tab"
									aria-selected="true"
								>
									sm
								</button>
								<button
									className="tab"
									type="button"
									role="tab"
									aria-selected="false"
								>
									tab
								</button>
							</div>
							<div className="tabs tabs-lift tabs-md" role="tablist">
								<button
									className="tab tab-active"
									type="button"
									role="tab"
									aria-selected="true"
								>
									md
								</button>
								<button
									className="tab"
									type="button"
									role="tab"
									aria-selected="false"
								>
									tab
								</button>
							</div>
							<div className="tabs tabs-lift tabs-lg" role="tablist">
								<button
									className="tab tab-active"
									type="button"
									role="tab"
									aria-selected="true"
								>
									lg
								</button>
								<button
									className="tab"
									type="button"
									role="tab"
									aria-selected="false"
								>
									tab
								</button>
							</div>
							<div className="tabs tabs-lift tabs-xl" role="tablist">
								<button
									className="tab tab-active"
									type="button"
									role="tab"
									aria-selected="true"
								>
									xl
								</button>
								<button
									className="tab"
									type="button"
									role="tab"
									aria-selected="false"
								>
									tab
								</button>
							</div>
						</div>
					</div>
				</div>
			</Section>

			<Section title="Alerts & Indicators">
				<div className="grid gap-3 md:grid-cols-2">
					<div className="alert alert-info">
						<Icon icon="mdi:information" className="text-lg" />
						<div>
							<h3 className="font-semibold">Info</h3>
							<div className="text-xs">General informational message.</div>
						</div>
					</div>
					<div className="alert alert-success">
						<Icon icon="mdi:check-circle" className="text-lg" />
						<div>
							<h3 className="font-semibold">Success</h3>
							<div className="text-xs">Everything looks good.</div>
						</div>
					</div>
					<div className="alert alert-warning">
						<Icon icon="mdi:alert" className="text-lg" />
						<div>
							<h3 className="font-semibold">Warning</h3>
							<div className="text-xs">Potential issue detected.</div>
						</div>
					</div>
					<div className="alert alert-error">
						<Icon icon="mdi:close-circle" className="text-lg" />
						<div>
							<h3 className="font-semibold">Error</h3>
							<div className="text-xs">Something went wrong.</div>
						</div>
					</div>
				</div>
			</Section>

			<Section title="Cards & Collapse">
				<div className="grid gap-4 md:grid-cols-2">
					<div className="card bg-base-100 shadow-sm">
						<div className="card-body">
							<h2 className="card-title">Simple card</h2>
							<p className="text-sm text-base-content/70">
								Use to preview typography and padding.
							</p>
							<div className="card-actions justify-end">
								<button className="btn btn-primary btn-sm" type="button">
									Action
								</button>
							</div>
						</div>
					</div>
					<div className="collapse collapse-arrow border border-base-300 bg-base-100">
						<input type="checkbox" defaultChecked />
						<div className="collapse-title text-md font-medium">
							Collapse example
						</div>
						<div className="collapse-content text-sm text-base-content/70">
							<p>Check padding, border, and arrow icon.</p>
						</div>
					</div>
				</div>
			</Section>

			<Section title="Table & List">
				<div className="grid gap-4 md:grid-cols-2">
					<table className="table table-zebra">
						<thead>
							<tr>
								<th>Item</th>
								<th>Status</th>
								<th>Owner</th>
							</tr>
						</thead>
						<tbody>
							<tr>
								<td>Alpha</td>
								<td>
									<span className="badge badge-success">ok</span>
								</td>
								<td>Ada</td>
							</tr>
							<tr>
								<td>Beta</td>
								<td>
									<span className="badge badge-warning">warning</span>
								</td>
								<td>Ben</td>
							</tr>
							<tr>
								<td>Gamma</td>
								<td>
									<span className="badge badge-error">failed</span>
								</td>
								<td>Cat</td>
							</tr>
						</tbody>
					</table>

					<ul className="steps steps-vertical">
						<li className="step step-primary">Queued</li>
						<li className="step step-primary">Processing</li>
						<li className="step">Verifying</li>
						<li className="step">Done</li>
					</ul>
				</div>
			</Section>
		</div>
	);
}
