<script lang="ts">
	import { Trash2, GripVertical, Filter } from 'lucide-svelte';
	import type { FilterResponse, FilterType } from '$lib/api/types';
	import AttachmentPointBadge from './AttachmentPointBadge.svelte';

	interface Props {
		filters: FilterResponse[];
		onDetach: (filterId: string) => void;
		isLoading?: boolean;
		emptyMessage?: string;
	}

	let { filters, onDetach, isLoading = false, emptyMessage = 'No filters attached' }: Props =
		$props();

	function getFilterTypeLabel(filterType: string): string {
		switch (filterType) {
			case 'header_mutation':
				return 'Header Mutation';
			case 'jwt_auth':
				return 'JWT Auth';
			case 'cors':
				return 'CORS';
			case 'rate_limit':
				return 'Rate Limit';
			case 'ext_authz':
				return 'External Auth';
			default:
				return filterType;
		}
	}

	function getFilterTypeBadgeColor(filterType: string): string {
		switch (filterType) {
			case 'header_mutation':
				return 'bg-blue-100 text-blue-800';
			case 'jwt_auth':
				return 'bg-green-100 text-green-800';
			case 'cors':
				return 'bg-purple-100 text-purple-800';
			case 'rate_limit':
				return 'bg-orange-100 text-orange-800';
			case 'ext_authz':
				return 'bg-red-100 text-red-800';
			default:
				return 'bg-gray-100 text-gray-800';
		}
	}
</script>

<div class="space-y-2">
	{#if isLoading}
		<div class="flex items-center justify-center py-4">
			<div class="animate-spin rounded-full h-5 w-5 border-b-2 border-blue-600"></div>
			<span class="ml-2 text-sm text-gray-600">Loading filters...</span>
		</div>
	{:else if filters.length === 0}
		<div class="flex items-center justify-center py-6 border-2 border-dashed border-gray-300 rounded-lg">
			<div class="text-center">
				<Filter class="h-8 w-8 text-gray-400 mx-auto mb-2" />
				<p class="text-sm text-gray-600">{emptyMessage}</p>
			</div>
		</div>
	{:else}
		{#each filters as filter, index}
			<div
				class="flex items-center justify-between p-3 bg-white border border-gray-200 rounded-lg hover:border-gray-300 transition-colors"
			>
				<div class="flex items-center gap-3">
					<div class="flex items-center justify-center w-6 h-6 text-gray-400">
						<span class="text-xs font-medium">{index + 1}</span>
					</div>
					<div class="flex flex-col">
						<div class="flex items-center gap-2">
							<span class="text-sm font-medium text-gray-900">{filter.name}</span>
							<span
								class="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded {getFilterTypeBadgeColor(
									filter.filterType
								)}"
							>
								{getFilterTypeLabel(filter.filterType)}
							</span>
						</div>
						{#if filter.description}
							<span class="text-xs text-gray-500 mt-0.5">{filter.description}</span>
						{/if}
					</div>
				</div>
				<button
					onclick={() => onDetach(filter.id)}
					class="p-1.5 text-red-600 hover:bg-red-50 rounded-md transition-colors"
					title="Detach filter"
				>
					<Trash2 class="h-4 w-4" />
				</button>
			</div>
		{/each}
	{/if}
</div>
