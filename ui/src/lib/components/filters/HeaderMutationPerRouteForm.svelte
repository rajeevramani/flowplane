<script lang="ts">
	import type { HeaderMutationPerRouteConfig } from "$lib/api/types";
	import HeaderMutationSection from "./HeaderMutationSection.svelte";

	interface Props {
		config: HeaderMutationPerRouteConfig | null;
		onUpdate: (config: HeaderMutationPerRouteConfig | null) => void;
	}

	let { config, onUpdate }: Props = $props();

	// Track if per-route header mutation is enabled
	let enabled = $state(config !== null);

	// Initialize state from config
	let requestHeadersToAdd = $state(config?.requestHeadersToAdd ?? []);
	let requestHeadersToRemove = $state(config?.requestHeadersToRemove ?? []);
	let responseHeadersToAdd = $state(config?.responseHeadersToAdd ?? []);
	let responseHeadersToRemove = $state(config?.responseHeadersToRemove ?? []);

	// Helper to propagate changes to parent
	function updateParent() {
		if (enabled) {
			onUpdate({
				requestHeadersToAdd,
				requestHeadersToRemove,
				responseHeadersToAdd,
				responseHeadersToRemove,
			});
		} else {
			onUpdate(null);
		}
	}

	function toggleEnabled() {
		enabled = !enabled;
		if (!enabled) {
			// Reset to empty when disabled
			requestHeadersToAdd = [];
			requestHeadersToRemove = [];
			responseHeadersToAdd = [];
			responseHeadersToRemove = [];
		}
		updateParent();
	}
</script>

<div class="space-y-4">
	<div class="flex items-center gap-2">
		<input
			type="checkbox"
			id="enable-per-route-mutation"
			checked={enabled}
			onchange={toggleEnabled}
			class="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
		/>
		<label
			for="enable-per-route-mutation"
			class="text-sm font-medium text-gray-700"
		>
			Enable per-route header mutation
		</label>
	</div>

	{#if enabled}
		<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
			<div class="flex items-start gap-2">
				<svg
					class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0"
					fill="none"
					stroke="currentColor"
					viewBox="0 0 24 24"
				>
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						stroke-width="2"
						d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
					/>
				</svg>
				<div class="text-xs text-blue-700">
					<p class="font-medium">Route-level Header Mutation</p>
					<p class="mt-1">
						These mutations apply only to this specific route and
						are processed in addition to any listener-level filters.
					</p>
				</div>
			</div>
		</div>

		<div class="space-y-4">
			<HeaderMutationSection
				headersToAdd={requestHeadersToAdd}
				headersToRemove={requestHeadersToRemove}
				onUpdateAdd={(headers) => {
					requestHeadersToAdd = headers;
					updateParent();
				}}
				onUpdateRemove={(headers) => {
					requestHeadersToRemove = headers;
					updateParent();
				}}
				sectionLabel="Request Headers"
				headerType="request"
			/>

			<HeaderMutationSection
				headersToAdd={responseHeadersToAdd}
				headersToRemove={responseHeadersToRemove}
				onUpdateAdd={(headers) => {
					responseHeadersToAdd = headers;
					updateParent();
				}}
				onUpdateRemove={(headers) => {
					responseHeadersToRemove = headers;
					updateParent();
				}}
				sectionLabel="Response Headers"
				headerType="response"
			/>
		</div>
	{:else}
		<p class="text-sm text-gray-500 italic">
			Enable to configure header mutations specific to this route.
		</p>
	{/if}
</div>
