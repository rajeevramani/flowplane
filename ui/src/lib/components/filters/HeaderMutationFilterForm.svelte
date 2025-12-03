<script lang="ts">
	import type { HeaderMutationConfig } from '$lib/api/types';
	import HeaderMutationSection from './HeaderMutationSection.svelte';

	interface Props {
		config: HeaderMutationConfig;
		onUpdate: (config: HeaderMutationConfig) => void;
	}

	let { config, onUpdate }: Props = $props();

	// Initialize empty arrays if not present
	let requestHeadersToAdd = $state(config.requestHeadersToAdd ?? []);
	let requestHeadersToRemove = $state(config.requestHeadersToRemove ?? []);
	let responseHeadersToAdd = $state(config.responseHeadersToAdd ?? []);
	let responseHeadersToRemove = $state(config.responseHeadersToRemove ?? []);

	// Helper to propagate changes to parent
	function updateParent() {
		onUpdate({
			requestHeadersToAdd,
			requestHeadersToRemove,
			responseHeadersToAdd,
			responseHeadersToRemove
		});
	}
</script>

<div class="space-y-4">
	<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
		<div class="flex items-start gap-2">
			<svg class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					stroke-width="2"
					d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
				/>
			</svg>
			<div class="text-xs text-blue-700">
				<p class="font-medium">Listener-level Header Mutation</p>
				<p class="mt-1">
					These mutations apply to all routes using this listener unless overridden at the route
					level.
				</p>
			</div>
		</div>
	</div>

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
