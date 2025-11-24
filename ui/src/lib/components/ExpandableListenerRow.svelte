<script lang="ts">
	import Badge from './Badge.svelte';
	import type { ListenerResponse, RouteResponse, ImportSummary } from '$lib/api/types';

	interface Props {
		listener: ListenerResponse;
		routes?: RouteResponse[];
		imports?: ImportSummary[];
		onDelete?: (listener: ListenerResponse) => void;
	}

	let { listener, routes = [], imports = [], onDelete }: Props = $props();

	let isExpanded = $state(false);

	function toggle() {
		isExpanded = !isExpanded;
	}

	function getImportSource(importId?: string): string {
		if (!importId) return 'Native';
		const imp = imports.find(i => i.id === importId);
		if (imp) {
			return imp.specVersion ? `${imp.specName} v${imp.specVersion}` : imp.specName;
		}
		return 'Unknown';
	}

	function getListenerConfig(): { address: string; port: number; filterChains: any[] } {
		try {
			const config = listener.config as any;
			return {
				address: config?.address || '0.0.0.0',
				port: config?.port || 0,
				filterChains: config?.filterChains || []
			};
		} catch {
			return { address: '0.0.0.0', port: 0, filterChains: [] };
		}
	}

	function getAssociatedRoutes(): RouteResponse[] {
		const config = getListenerConfig();
		const routeConfigNames = new Set<string>();

		for (const fc of config.filterChains) {
			for (const f of fc.filters || []) {
				if (f.routeConfigName) {
					routeConfigNames.add(f.routeConfigName);
				}
			}
		}

		return routes.filter(r => routeConfigNames.has(r.name));
	}

	function getFilterInfo(filter: any): { type: string; details: string } {
		if (filter.type === 'httpConnectionManager' || filter.routeConfigName) {
			return {
				type: 'HTTP Connection Manager',
				details: filter.routeConfigName ? `Route: ${filter.routeConfigName}` : 'Inline routes'
			};
		}
		if (filter.type === 'tcpProxy') {
			return {
				type: 'TCP Proxy',
				details: filter.cluster ? `Cluster: ${filter.cluster}` : ''
			};
		}
		return { type: filter.type || 'Unknown', details: '' };
	}

	function hasTls(filterChain: any): boolean {
		return !!filterChain.tlsContext;
	}

	function handleDelete(e: Event) {
		e.stopPropagation();
		if (onDelete && confirm(`Delete listener "${listener.name}"?`)) {
			onDelete(listener);
		}
	}

	const config = $derived(getListenerConfig());
	const associatedRoutes = $derived(getAssociatedRoutes());
</script>

<!-- Row Header -->
<button
	type="button"
	onclick={toggle}
	class="w-full flex items-center justify-between py-3 px-4 hover:bg-gray-50 transition-colors text-left group border-b border-gray-100"
>
	<div class="flex items-center gap-6 flex-1 min-w-0">
		<!-- Expand Icon -->
		<svg
			class="h-4 w-4 text-gray-400 transition-transform flex-shrink-0 {isExpanded ? 'rotate-90' : ''}"
			fill="none"
			stroke="currentColor"
			viewBox="0 0 24 24"
		>
			<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
		</svg>

		<!-- Name -->
		<div class="w-48 min-w-0">
			<span class="font-medium text-gray-900 truncate block">{listener.name}</span>
		</div>

		<!-- Team -->
		<div class="w-24">
			<Badge variant="blue">{listener.team}</Badge>
		</div>

		<!-- Address:Port -->
		<div class="w-40 text-sm text-gray-600 font-mono">
			{config.address}:{config.port}
		</div>

		<!-- Protocol -->
		<div class="w-24 text-sm text-gray-600">
			{listener.protocol}
		</div>

		<!-- Filter Chains count -->
		<div class="w-28 text-sm text-gray-600">
			{config.filterChains.length} chain{config.filterChains.length !== 1 ? 's' : ''}
		</div>

		<!-- Source -->
		<div class="flex-1 text-sm text-gray-500 truncate">
			{getImportSource(listener.importId)}
		</div>
	</div>

	<!-- Actions -->
	<div class="flex items-center gap-2">
		{#if onDelete}
			<button
				onclick={handleDelete}
				class="p-1.5 text-gray-400 hover:text-red-600 rounded opacity-0 group-hover:opacity-100 transition-opacity"
				title="Delete"
			>
				<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
				</svg>
			</button>
		{/if}
	</div>
</button>

<!-- Expanded Content -->
{#if isExpanded}
	<div class="bg-gray-50 border-b border-gray-200 px-4 py-4">
		<!-- Basic Info -->
		<div class="grid grid-cols-4 gap-4 mb-4">
			<div>
				<label class="block text-xs font-medium text-gray-500 uppercase">Address</label>
				<p class="mt-1 text-sm text-gray-900 font-mono">{config.address}</p>
			</div>
			<div>
				<label class="block text-xs font-medium text-gray-500 uppercase">Port</label>
				<p class="mt-1 text-sm text-gray-900 font-mono">{config.port}</p>
			</div>
			<div>
				<label class="block text-xs font-medium text-gray-500 uppercase">Protocol</label>
				<p class="mt-1 text-sm text-gray-900">{listener.protocol}</p>
			</div>
			<div>
				<label class="block text-xs font-medium text-gray-500 uppercase">Version</label>
				<p class="mt-1 text-sm text-gray-900">v{listener.version}</p>
			</div>
		</div>

		<!-- Filter Chains -->
		{#if config.filterChains.length > 0}
			<div class="mb-4">
				<h4 class="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">Filter Chains</h4>
				<div class="bg-white rounded-lg border border-gray-200 overflow-hidden">
					<table class="min-w-full divide-y divide-gray-200">
						<thead class="bg-gray-50">
							<tr>
								<th class="px-3 py-2 text-left text-xs font-medium text-gray-500 uppercase">Name</th>
								<th class="px-3 py-2 text-left text-xs font-medium text-gray-500 uppercase">TLS</th>
								<th class="px-3 py-2 text-left text-xs font-medium text-gray-500 uppercase">Filters</th>
							</tr>
						</thead>
						<tbody class="divide-y divide-gray-100">
							{#each config.filterChains as fc, fcIndex}
								<tr class="hover:bg-gray-50">
									<td class="px-3 py-2 text-sm text-gray-900">
										{fc.name || `Chain ${fcIndex + 1}`}
									</td>
									<td class="px-3 py-2">
										{#if hasTls(fc)}
											<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-green-100 text-green-700">
												TLS
											</span>
										{:else}
											<span class="text-xs text-gray-400">-</span>
										{/if}
									</td>
									<td class="px-3 py-2">
										<div class="flex flex-wrap gap-1">
											{#each fc.filters || [] as filter}
												{@const info = getFilterInfo(filter)}
												<span class="inline-flex items-center px-2 py-0.5 rounded text-xs bg-purple-50 text-purple-700" title={info.details}>
													{info.type}
												</span>
											{/each}
										</div>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			</div>
		{/if}

		<!-- Associated Routes -->
		{#if associatedRoutes.length > 0}
			<div>
				<h4 class="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">Associated Routes</h4>
				<div class="flex flex-wrap gap-2">
					{#each associatedRoutes as route}
						<span class="inline-flex items-center gap-1.5 px-2.5 py-1 bg-blue-50 text-blue-700 text-xs rounded-md border border-blue-200">
							<svg class="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 7l5 5m0 0l-5 5m5-5H6" />
							</svg>
							{route.name}
						</span>
					{/each}
				</div>
			</div>
		{/if}
	</div>
{/if}
