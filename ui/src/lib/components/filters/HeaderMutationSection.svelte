<script lang="ts">
	import type { HeaderMutationEntry } from '$lib/api/types';
	import HeaderAddList from './HeaderAddList.svelte';
	import HeaderRemoveList from './HeaderRemoveList.svelte';

	interface Props {
		headersToAdd: HeaderMutationEntry[];
		headersToRemove: string[];
		onUpdateAdd: (headers: HeaderMutationEntry[]) => void;
		onUpdateRemove: (headers: string[]) => void;
		sectionLabel: string;
		headerType: 'request' | 'response';
	}

	let { headersToAdd, headersToRemove, onUpdateAdd, onUpdateRemove, sectionLabel, headerType }:
		Props = $props();
</script>

<div class="space-y-4 rounded-lg border border-gray-200 bg-gray-50 p-4">
	<h4 class="text-sm font-semibold text-gray-700">{sectionLabel}</h4>

	<div class="space-y-4">
		<HeaderAddList
			headers={headersToAdd}
			onHeadersChange={onUpdateAdd}
			label="Headers to Add/Modify"
			{headerType}
		/>

		<HeaderRemoveList
			headers={headersToRemove}
			onHeadersChange={onUpdateRemove}
			label="Headers to Remove"
			{headerType}
		/>
	</div>
</div>
