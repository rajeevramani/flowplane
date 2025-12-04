<script lang="ts">
	import type { HeaderMutationConfig, HeaderMutationEntry } from '$lib/api/types';
	import HeaderAddList from './HeaderAddList.svelte';
	import HeaderRemoveList from './HeaderRemoveList.svelte';
	import { Info, ArrowUpFromLine, ArrowDownToLine, Plus, Minus } from 'lucide-svelte';

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
	<!-- Info Box -->
	<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
		<div class="flex items-start gap-2">
			<Info class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0" />
			<div class="text-xs text-blue-700">
				<p class="font-medium">Header Mutation</p>
				<p class="mt-1">
					Modify HTTP headers on requests and responses. Add headers to inject values,
					or remove headers to strip sensitive information before forwarding.
				</p>
			</div>
		</div>
	</div>

	<!-- Request Headers to Add -->
	<div class="border-b border-gray-200 pb-6">
		<div class="flex items-center gap-2 mb-2">
			<ArrowUpFromLine class="w-4 h-4 text-blue-600" />
			<Plus class="w-3 h-3 text-green-600" />
		</div>
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
		<div class="flex items-center gap-2 mb-2">
			<ArrowUpFromLine class="w-4 h-4 text-blue-600" />
			<Minus class="w-3 h-3 text-red-600" />
		</div>
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
		<div class="flex items-center gap-2 mb-2">
			<ArrowDownToLine class="w-4 h-4 text-purple-600" />
			<Plus class="w-3 h-3 text-green-600" />
		</div>
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
		<div class="flex items-center gap-2 mb-2">
			<ArrowDownToLine class="w-4 h-4 text-purple-600" />
			<Minus class="w-3 h-3 text-red-600" />
		</div>
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
