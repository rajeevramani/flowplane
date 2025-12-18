<script lang="ts">
	import type { AttachmentPoint, FilterType } from '$lib/api/types';
	import { getAllowedAttachmentPointsDisplay } from '$lib/utils/filter-attachment';

	interface Props {
		filterType: FilterType;
		variant?: 'default' | 'compact';
	}

	let { filterType, variant = 'default' }: Props = $props();

	const displayText = $derived(getAllowedAttachmentPointsDisplay(filterType));
</script>

{#if variant === 'compact'}
	<span class="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded bg-gray-100 text-gray-700 whitespace-nowrap">
		{displayText}
	</span>
{:else}
	<span class="inline-flex items-center px-2.5 py-1 text-xs font-medium rounded-md bg-blue-50 text-blue-700 border border-blue-200 whitespace-nowrap">
		Can attach to: {displayText}
	</span>
{/if}
