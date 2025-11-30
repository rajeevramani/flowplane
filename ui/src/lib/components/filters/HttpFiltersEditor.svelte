<script lang="ts">
	import type { HttpFilterConfigEntry, HeaderMutationConfig } from '$lib/api/types';
	import { ChevronDown, ChevronRight } from 'lucide-svelte';
	import HeaderMutationFilterForm from './HeaderMutationFilterForm.svelte';

	interface Props {
		filters: HttpFilterConfigEntry[];
		onUpdate: (filters: HttpFilterConfigEntry[]) => void;
	}

	let { filters, onUpdate }: Props = $props();

	// Track which filter is expanded for editing
	let expandedFilterIndex = $state<number | null>(null);

	function toggleFilter(index: number) {
		if (expandedFilterIndex === index) {
			expandedFilterIndex = null;
		} else {
			expandedFilterIndex = index;
		}
	}

	function addFilter(filterType: string) {
		const newFilter: HttpFilterConfigEntry = {
			filter:
				filterType === 'header_mutation'
					? {
							type: 'header_mutation',
							config: {
								requestHeadersToAdd: [],
								requestHeadersToRemove: [],
								responseHeadersToAdd: [],
								responseHeadersToRemove: []
							}
						}
					: { type: filterType as 'cors' | 'local_rate_limit' }
		};

		// Insert before router filter (router should always be last)
		const routerIndex = filters.findIndex((f) => f.filter.type === 'router');
		if (routerIndex >= 0) {
			const newFilters = [...filters];
			newFilters.splice(routerIndex, 0, newFilter);
			onUpdate(newFilters);
		} else {
			onUpdate([...filters, newFilter]);
		}
	}

	function removeFilter(index: number) {
		onUpdate(filters.filter((_, i) => i !== index));
	}

	function updateFilter(index: number, filter: HttpFilterConfigEntry) {
		onUpdate(filters.map((f, i) => (i === index ? filter : f)));
	}

	function getFilterDisplayName(filter: HttpFilterConfigEntry): string {
		const type = filter.filter.type;
		switch (type) {
			case 'router':
				return 'Router';
			case 'header_mutation':
				return 'Header Mutation';
			case 'cors':
				return 'CORS';
			case 'local_rate_limit':
				return 'Local Rate Limit';
			case 'jwt_authn':
				return 'JWT Authentication';
			case 'rate_limit':
				return 'Rate Limit';
			case 'health_check':
				return 'Health Check';
			default:
				return 'Filter';
		}
	}

	function getFilterSummary(filter: HttpFilterConfigEntry): string {
		if (filter.filter.type === 'header_mutation') {
			const config = filter.filter.config;
			const parts: string[] = [];
			const reqAdd = config.requestHeadersToAdd?.length || 0;
			const reqRemove = config.requestHeadersToRemove?.length || 0;
			const respAdd = config.responseHeadersToAdd?.length || 0;
			const respRemove = config.responseHeadersToRemove?.length || 0;

			if (reqAdd > 0) parts.push(`${reqAdd} req add`);
			if (reqRemove > 0) parts.push(`${reqRemove} req remove`);
			if (respAdd > 0) parts.push(`${respAdd} resp add`);
			if (respRemove > 0) parts.push(`${respRemove} resp remove`);

			return parts.length > 0 ? parts.join(', ') : 'No mutations configured';
		}
		return '';
	}

	function handleHeaderMutationUpdate(index: number, config: HeaderMutationConfig) {
		const updatedFilter: HttpFilterConfigEntry = {
			...filters[index],
			filter: {
				type: 'header_mutation',
				config
			}
		};
		updateFilter(index, updatedFilter);
	}
</script>

<div class="space-y-3">
	<div class="flex items-center justify-between">
		<label class="block text-sm font-medium text-gray-700">HTTP Filters</label>
		<div class="relative inline-block text-left">
			<select
				onchange={(e) => {
					const select = e.target as HTMLSelectElement;
					if (select.value) {
						addFilter(select.value);
						select.value = '';
					}
				}}
				class="rounded-md border border-gray-300 bg-white px-3 py-1.5 text-xs font-medium text-blue-600 hover:bg-gray-50 focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
			>
				<option value="">+ Add Filter</option>
				<option value="header_mutation">Header Mutation</option>
				<option value="cors">CORS</option>
				<option value="local_rate_limit">Local Rate Limit</option>
				<option value="jwt_authn">JWT Authentication</option>
			</select>
		</div>
	</div>

	{#if filters.length === 0}
		<div class="text-sm text-gray-500 italic p-4 bg-gray-50 rounded-md border border-gray-200">
			No HTTP filters configured. The router filter will be added automatically.
		</div>
	{:else}
		<div class="space-y-2">
			{#each filters as filter, index}
				{@const isRouter = filter.filter.type === 'router'}
				{@const isExpanded = expandedFilterIndex === index}
				{@const canEdit = filter.filter.type === 'header_mutation'}
				{@const canDelete = !isRouter}

				<div class="border border-gray-200 rounded-md bg-white overflow-hidden">
					<!-- Filter Header -->
					<div
						class="p-3 flex items-center justify-between hover:bg-gray-50 transition-colors {canEdit
							? 'cursor-pointer'
							: ''}"
						onclick={() => canEdit && toggleFilter(index)}
					>
						<div class="flex-1">
							<div class="flex items-center gap-2">
								<span class="text-sm font-medium text-gray-900">
									{getFilterDisplayName(filter)}
								</span>
								{#if isRouter}
									<span
										class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-blue-100 text-blue-800"
									>
										Required
									</span>
								{/if}
							</div>
							{#if filter.filter.type === 'header_mutation'}
								<p class="text-xs text-gray-500 mt-1">{getFilterSummary(filter)}</p>
							{/if}
						</div>

						<div class="flex items-center gap-2">
							{#if canDelete}
								<button
									type="button"
									onclick={(e) => {
										e.stopPropagation();
										removeFilter(index);
									}}
									class="p-1.5 rounded text-gray-400 hover:bg-red-50 hover:text-red-500 transition-colors"
									title="Remove filter"
								>
									<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
										<path
											stroke-linecap="round"
											stroke-linejoin="round"
											stroke-width="2"
											d="M6 18L18 6M6 6l12 12"
										/>
									</svg>
								</button>
							{/if}

							{#if canEdit}
								<button
									type="button"
									class="p-1.5 rounded hover:bg-gray-200 text-gray-600 transition-colors"
									title={isExpanded ? 'Collapse' : 'Expand to edit'}
								>
									{#if isExpanded}
										<ChevronDown class="h-4 w-4" />
									{:else}
										<ChevronRight class="h-4 w-4" />
									{/if}
								</button>
							{/if}
						</div>
					</div>

					<!-- Expanded Filter Configuration -->
					{#if isExpanded && filter.filter.type === 'header_mutation'}
						<div class="border-t border-gray-200 p-4 bg-gray-50">
							<HeaderMutationFilterForm
								config={filter.filter.config}
								onUpdate={(config) => handleHeaderMutationUpdate(index, config)}
							/>
						</div>
					{/if}
				</div>
			{/each}
		</div>
	{/if}

	<div class="rounded-lg border border-gray-200 bg-gray-50 p-3">
		<p class="text-xs text-gray-600">
			<strong>Note:</strong> The Router filter is automatically added as the last filter in the chain
			and cannot be removed. Other filters will be processed in the order shown above.
		</p>
	</div>
</div>
