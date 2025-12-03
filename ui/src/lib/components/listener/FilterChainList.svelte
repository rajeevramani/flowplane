<script lang="ts">
	import type { ListenerFilterChainInput, ListenerTlsContextInput, HttpFilterConfigEntry } from '$lib/api/types';
	import { Lock, LockOpen, ChevronDown, ChevronRight } from 'lucide-svelte';
	import TlsConfigForm from './TlsConfigForm.svelte';
	import HttpFiltersEditor from '../filters/HttpFiltersEditor.svelte';

	interface Props {
		filterChains: ListenerFilterChainInput[];
		onFilterChainsChange: (chains: ListenerFilterChainInput[]) => void;
		compact?: boolean;
	}

	let { filterChains, onFilterChainsChange, compact = false }: Props = $props();

	// Track which filter chain is expanded for TLS editing
	let expandedChainIndex = $state<number | null>(null);

	function toggleChain(index: number) {
		if (expandedChainIndex === index) {
			expandedChainIndex = null;
		} else {
			expandedChainIndex = index;
		}
	}

	function handleTlsContextChange(index: number, context: ListenerTlsContextInput | null) {
		const updatedChains = filterChains.map((chain, i) =>
			i === index
				? { ...chain, tlsContext: context || undefined }
				: chain
		);
		onFilterChainsChange(updatedChains);
	}

	function handleHttpFiltersChange(chainIndex: number, filterIndex: number, httpFilters: HttpFilterConfigEntry[]) {
		const updatedChains = filterChains.map((chain, ci) => {
			if (ci === chainIndex) {
				const updatedFilters = chain.filters.map((filter, fi) => {
					if (fi === filterIndex && 'type' in filter && filter.type === 'httpConnectionManager') {
						return {
							...filter,
							httpFilters
						};
					}
					return filter;
				});
				return {
					...chain,
					filters: updatedFilters
				};
			}
			return chain;
		});
		onFilterChainsChange(updatedChains);
	}

	function getFilterDisplayName(filter: ListenerFilterChainInput['filters'][0]): string {
		switch (filter.type) {
			case 'httpConnectionManager':
				return 'HttpConnectionManager';
			case 'tcpProxy':
				return 'TcpProxy';
			default:
				// Fallback that TypeScript knows won't be reached
				return (filter as { name?: string }).name || 'Filter';
		}
	}

	function getFilterTarget(filter: ListenerFilterChainInput['filters'][0]): string | null {
		if ('type' in filter && filter.type === 'httpConnectionManager') {
			return filter.routeConfigName || null;
		} else if ('type' in filter && filter.type === 'tcpProxy') {
			return filter.cluster || null;
		}
		return null;
	}

	// Get HTTP filters from an HttpConnectionManager filter, excluding the router
	function getHttpFilters(filter: ListenerFilterChainInput['filters'][0]): HttpFilterConfigEntry[] {
		if ('type' in filter && filter.type === 'httpConnectionManager' && filter.httpFilters) {
			// Filter out the router filter for display
			return filter.httpFilters.filter((hf) => hf.filter.type !== 'router');
		}
		return [];
	}

	// Get a display name for an HTTP filter type
	function getHttpFilterTypeName(filterType: string): string {
		switch (filterType) {
			case 'header_mutation':
				return 'Header Mutation';
			case 'cors':
				return 'CORS';
			case 'local_rate_limit':
				return 'Rate Limit (Local)';
			case 'jwt_authn':
				return 'JWT Auth';
			case 'rate_limit':
				return 'Rate Limit';
			case 'health_check':
				return 'Health Check';
			default:
				return filterType;
		}
	}

	// Get badge color for an HTTP filter type
	function getHttpFilterBadgeColor(filterType: string): string {
		switch (filterType) {
			case 'header_mutation':
				return 'bg-blue-100 text-blue-700';
			case 'cors':
				return 'bg-purple-100 text-purple-700';
			case 'jwt_authn':
				return 'bg-green-100 text-green-700';
			case 'rate_limit':
			case 'local_rate_limit':
				return 'bg-orange-100 text-orange-700';
			case 'health_check':
				return 'bg-gray-100 text-gray-700';
			default:
				return 'bg-gray-100 text-gray-700';
		}
	}
</script>

<div class="space-y-3">
	{#if filterChains.length === 0}
		<div class="text-sm text-gray-500 italic p-4 bg-gray-50 rounded-md border border-gray-200">
			No filter chains configured
		</div>
	{:else}
		{#each filterChains as chain, index}
			{@const hasTls = chain.tlsContext !== undefined && chain.tlsContext !== null}
			{@const isExpanded = expandedChainIndex === index}
			{@const filterCount = chain.filters?.length || 0}

			<div class="border border-gray-200 rounded-md bg-white overflow-hidden">
				<!-- Chain Header -->
				<div class="p-3 bg-gray-50">
					<div class="flex items-center justify-between">
						<div class="flex-1">
							<div class="flex items-center gap-2">
								<span class="text-sm font-medium text-gray-900">
									{chain.name || `Filter Chain ${index + 1}`}
								</span>
								<span class="text-xs text-gray-500">
									{filterCount} filter{filterCount !== 1 ? 's' : ''}
								</span>
							</div>
							<div class="mt-1 flex items-center gap-2">
								{#if hasTls}
									<div class="flex items-center gap-1 text-green-600 text-xs">
										<Lock class="h-3 w-3" />
										<span>TLS Enabled</span>
									</div>
								{:else}
									<div class="flex items-center gap-1 text-gray-400 text-xs">
										<LockOpen class="h-3 w-3" />
										<span>TLS Disabled</span>
									</div>
								{/if}
							</div>
						</div>
						<button
							type="button"
							onclick={() => toggleChain(index)}
							class="p-1.5 rounded hover:bg-gray-200 text-gray-600 transition-colors"
							title={isExpanded ? 'Collapse' : 'Expand to edit TLS'}
						>
							{#if isExpanded}
								<ChevronDown class="h-4 w-4" />
							{:else}
								<ChevronRight class="h-4 w-4" />
							{/if}
						</button>
					</div>
				</div>

				<!-- Filters List (always visible) -->
				{#if filterCount > 0 && !compact}
					<div class="px-3 py-2 border-t border-gray-100">
						<div class="space-y-2">
							{#each chain.filters as filter}
								{@const displayName = getFilterDisplayName(filter)}
								{@const target = getFilterTarget(filter)}
								{@const httpFilters = getHttpFilters(filter)}
								<div class="space-y-1">
									<div class="text-xs text-gray-600 flex items-center gap-1">
										<span class="text-gray-400">•</span>
										<span>{displayName}</span>
										{#if target}
											<span class="text-gray-400">→</span>
											<span class="font-mono text-gray-700">{target}</span>
										{/if}
									</div>
									<!-- Show HTTP filters inline -->
									{#if httpFilters.length > 0}
										<div class="ml-4 flex flex-wrap gap-1">
											{#each httpFilters as httpFilter}
												<span class="inline-flex items-center px-1.5 py-0.5 rounded text-xs {getHttpFilterBadgeColor(httpFilter.filter.type)}">
													{getHttpFilterTypeName(httpFilter.filter.type)}
												</span>
											{/each}
										</div>
									{/if}
								</div>
							{/each}
						</div>
					</div>
				{/if}

				<!-- Expanded Configuration (TLS + HTTP Filters) -->
				{#if isExpanded}
					<div class="border-t border-gray-200 space-y-4">
						<!-- TLS Configuration -->
						<div class="p-4 bg-blue-50">
							<h4 class="text-sm font-medium text-gray-700 mb-3">TLS Configuration</h4>
							<TlsConfigForm
								tlsContext={chain.tlsContext || null}
								onTlsContextChange={(context) => handleTlsContextChange(index, context)}
								compact={compact}
							/>
						</div>

						<!-- HTTP Filters Configuration (only for HttpConnectionManager) -->
						{#each chain.filters as filter, filterIndex}
							{#if 'type' in filter && filter.type === 'httpConnectionManager'}
								<div class="p-4 bg-gray-50">
									<HttpFiltersEditor
										filters={filter.httpFilters || []}
										onUpdate={(httpFilters) => handleHttpFiltersChange(index, filterIndex, httpFilters)}
									/>
								</div>
							{/if}
						{/each}
					</div>
				{/if}
			</div>
		{/each}
	{/if}
</div>
