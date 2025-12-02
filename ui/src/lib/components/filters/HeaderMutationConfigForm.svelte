<script lang="ts">
	import type { HeaderMutationConfig, HeaderMutationEntry } from '$lib/api/types';
	import HeaderAddList from './HeaderAddList.svelte';
	import HeaderRemoveList from './HeaderRemoveList.svelte';

	interface Props {
		config: HeaderMutationConfig;
		onConfigChange: (config: HeaderMutationConfig) => void;
	}

	let { config, onConfigChange }: Props = $props();

	function handleRequestHeadersToAddChange(headers: HeaderMutationEntry[]) {
		onConfigChange({
			...config,
			requestHeadersToAdd: headers
		});
	}

	function handleRequestHeadersToRemoveChange(headers: string[]) {
		onConfigChange({
			...config,
			requestHeadersToRemove: headers
		});
	}

	function handleResponseHeadersToAddChange(headers: HeaderMutationEntry[]) {
		onConfigChange({
			...config,
			responseHeadersToAdd: headers
		});
	}

	function handleResponseHeadersToRemoveChange(headers: string[]) {
		onConfigChange({
			...config,
			responseHeadersToRemove: headers
		});
	}
</script>

<div class="space-y-6">
	<!-- Request Headers to Add -->
	<div class="border-b border-gray-200 pb-6">
		<HeaderAddList
			headers={config.requestHeadersToAdd || []}
			onHeadersChange={handleRequestHeadersToAddChange}
			label="Request Headers to Add"
			headerType="request"
		/>
		<p class="text-xs text-gray-500 mt-2">
			Headers to add to incoming requests before forwarding to upstream
		</p>
	</div>

	<!-- Request Headers to Remove -->
	<div class="border-b border-gray-200 pb-6">
		<HeaderRemoveList
			headers={config.requestHeadersToRemove || []}
			onHeadersChange={handleRequestHeadersToRemoveChange}
			label="Request Headers to Remove"
			headerType="request"
		/>
		<p class="text-xs text-gray-500 mt-2">
			Headers to remove from incoming requests before forwarding
		</p>
	</div>

	<!-- Response Headers to Add -->
	<div class="border-b border-gray-200 pb-6">
		<HeaderAddList
			headers={config.responseHeadersToAdd || []}
			onHeadersChange={handleResponseHeadersToAddChange}
			label="Response Headers to Add"
			headerType="response"
		/>
		<p class="text-xs text-gray-500 mt-2">
			Headers to add to responses before sending to client
		</p>
	</div>

	<!-- Response Headers to Remove -->
	<div>
		<HeaderRemoveList
			headers={config.responseHeadersToRemove || []}
			onHeadersChange={handleResponseHeadersToRemoveChange}
			label="Response Headers to Remove"
			headerType="response"
		/>
		<p class="text-xs text-gray-500 mt-2">
			Headers to remove from responses before sending to client
		</p>
	</div>
</div>
