<script lang="ts">
	import type { ListenerFilterChainInput, ListenerTlsContextInput } from '$lib/api/types';
	import { Lock, LockOpen, ChevronDown, ChevronRight } from 'lucide-svelte';
	import TlsConfigForm from './TlsConfigForm.svelte';

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

	function getFilterDisplayName(filter: ListenerFilterChainInput['filters'][0]): string {
		if ('type' in filter) {
			if (filter.type === 'httpConnectionManager') {
				return 'HttpConnectionManager';
			} else if (filter.type === 'tcpProxy') {
				return 'TcpProxy';
			}
		}
		return filter.name || 'Filter';
	}

	function getFilterTarget(filter: ListenerFilterChainInput['filters'][0]): string | null {
		if ('type' in filter && filter.type === 'httpConnectionManager') {
			return filter.routeConfigName || null;
		} else if ('type' in filter && filter.type === 'tcpProxy') {
			return filter.cluster || null;
		}
		return null;
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
						<div class="space-y-1">
							{#each chain.filters as filter}
								{@const displayName = getFilterDisplayName(filter)}
								{@const target = getFilterTarget(filter)}
								<div class="text-xs text-gray-600 flex items-center gap-1">
									<span class="text-gray-400">•</span>
									<span>{displayName}</span>
									{#if target}
										<span class="text-gray-400">→</span>
										<span class="font-mono text-gray-700">{target}</span>
									{/if}
								</div>
							{/each}
						</div>
					</div>
				{/if}

				<!-- TLS Configuration (expandable) -->
				{#if isExpanded}
					<div class="p-4 border-t border-gray-200 bg-blue-50">
						<h4 class="text-sm font-medium text-gray-700 mb-3">TLS Configuration</h4>
						<TlsConfigForm
							tlsContext={chain.tlsContext || null}
							onTlsContextChange={(context) => handleTlsContextChange(index, context)}
							compact={compact}
						/>
					</div>
				{/if}
			</div>
		{/each}
	{/if}

	<!-- Phase 1 Note -->
	{#if !compact && filterChains.length > 0}
		<div class="text-xs text-gray-500 italic p-3 bg-gray-50 rounded-md">
			<strong>Note:</strong> In this version, you can only edit TLS configuration for existing filter chains.
			Adding/removing filter chains and configuring HTTP filters will be available in a future update.
		</div>
	{/if}
</div>
