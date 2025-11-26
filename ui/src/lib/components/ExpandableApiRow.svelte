<script lang="ts">
	import Badge from './Badge.svelte';
	import type { RouteResponse, ClusterResponse, ListenerResponse, ImportSummary } from '$lib/api/types';

	interface VirtualHost {
		name?: string;
		domains?: string[];
		routes?: Array<{
			name?: string;
			match?: {
				path?: { type?: string; value?: string; Prefix?: string; Exact?: string; Regex?: string };
				headers?: Array<{ name: string; value?: string }>;
			};
			action?: { type?: string; cluster?: string };
		}>;
	}

	interface Props {
		route: RouteResponse;
		imports?: ImportSummary[];
		clusters?: ClusterResponse[];
		listeners?: ListenerResponse[];
		onDelete?: (route: RouteResponse) => void;
	}

	let { route, imports = [], clusters = [], listeners = [], onDelete }: Props = $props();

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

	function extractVirtualHosts(): VirtualHost[] {
		try {
			const config = route.config as { virtualHosts?: VirtualHost[] };
			return config?.virtualHosts || [];
		} catch {
			return [];
		}
	}

	function getPathType(path: any): string {
		if (!path) return 'prefix';
		if (path.Exact || path.type === 'exact') return 'exact';
		if (path.Regex || path.type === 'regex') return 'regex';
		if (path.type === 'template') return 'template';
		return 'prefix';
	}

	function getPathValue(path: any): string {
		if (!path) return '/';
		return path.Prefix || path.Exact || path.Regex || path.value || path.template || '/';
	}

	function getRouteVerbs(r: any): string[] {
		// Check if there are header matchers for :method
		const headers = r.match?.headers || [];
		const methodHeader = headers.find((h: any) => h.name === ':method');
		if (methodHeader && methodHeader.value) {
			return [methodHeader.value];
		}
		return ['ALL'];
	}

	function getClusterEndpoint(clusterName: string): string {
		const cluster = clusters.find(c => c.name === clusterName);
		if (!cluster) return '-';
		try {
			const config = cluster.config as any;
			if (config?.endpoints && Array.isArray(config.endpoints) && config.endpoints.length > 0) {
				const ep = config.endpoints[0];
				return `${ep.host || '?'}:${ep.port || '?'}`;
			}
		} catch {
			// ignore
		}
		return '-';
	}

	function getAssociatedListeners(): ListenerResponse[] {
		return listeners.filter(l => {
			try {
				const config = l.config as any;
				return config?.filterChains?.some((fc: any) =>
					fc.filters?.some((f: any) => f.routeConfigName === route.name)
				);
			} catch {
				return false;
			}
		});
	}

	function handleDelete(e: Event) {
		e.stopPropagation();
		if (onDelete && confirm(`Delete route "${route.name}"?`)) {
			onDelete(route);
		}
	}

	const virtualHosts = $derived(extractVirtualHosts());
	const associatedListeners = $derived(getAssociatedListeners());

	// Count total routes across all virtual hosts
	const totalRoutes = $derived(virtualHosts.reduce((sum, vh) => sum + (vh.routes?.length || 0), 0));
</script>

<!-- Row Header (always visible) -->
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
			<span class="font-medium text-gray-900 truncate block">{route.name}</span>
		</div>

		<!-- Team -->
		<div class="w-24">
			<Badge variant="blue">{route.team}</Badge>
		</div>

		<!-- Domains count -->
		<div class="w-32 text-sm text-gray-600">
			{virtualHosts.length} domain{virtualHosts.length !== 1 ? 's' : ''}
		</div>

		<!-- Routes count -->
		<div class="w-28 text-sm text-gray-600">
			{totalRoutes} route{totalRoutes !== 1 ? 's' : ''}
		</div>

		<!-- Source -->
		<div class="flex-1 text-sm text-gray-500 truncate">
			{getImportSource(route.importId)}
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
		<!-- Associated Listeners -->
		{#if associatedListeners.length > 0}
			<div class="mb-4">
				<h4 class="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">Listeners</h4>
				<div class="flex flex-wrap gap-2">
					{#each associatedListeners as listener}
						{@const config = listener.config as any}
						<span class="inline-flex items-center gap-1.5 px-2.5 py-1 bg-orange-50 text-orange-700 text-xs rounded-md border border-orange-200">
							<svg class="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
							</svg>
							{listener.name}
							<span class="text-orange-500">({config?.address || '0.0.0.0'}:{config?.port || '?'})</span>
						</span>
					{/each}
				</div>
			</div>
		{/if}

		<!-- Virtual Hosts / Domains -->
		{#each virtualHosts as vh, vhIndex}
			<div class="mb-4 last:mb-0">
				<!-- Domain Header -->
				<div class="flex items-center gap-2 mb-2">
					<h4 class="text-xs font-semibold text-gray-500 uppercase tracking-wide">Domain</h4>
					<span class="text-sm font-medium text-gray-900">{vh.domains?.join(', ') || '*'}</span>
				</div>

				<!-- Routes Table -->
				{#if vh.routes && vh.routes.length > 0}
					<div class="bg-white rounded-lg border border-gray-200 overflow-hidden">
						<table class="min-w-full divide-y divide-gray-200">
							<thead class="bg-gray-50">
								<tr>
									<th class="px-3 py-2 text-left text-xs font-medium text-gray-500 uppercase w-20">Verb</th>
									<th class="px-3 py-2 text-left text-xs font-medium text-gray-500 uppercase w-20">Type</th>
									<th class="px-3 py-2 text-left text-xs font-medium text-gray-500 uppercase">Path</th>
									<th class="px-3 py-2 text-left text-xs font-medium text-gray-500 uppercase">Target Cluster</th>
									<th class="px-3 py-2 text-left text-xs font-medium text-gray-500 uppercase">Endpoint</th>
								</tr>
							</thead>
							<tbody class="divide-y divide-gray-100">
								{#each vh.routes as r, rIndex}
									{@const verbs = getRouteVerbs(r)}
									{@const pathType = getPathType(r.match?.path)}
									{@const pathValue = getPathValue(r.match?.path)}
									{@const targetCluster = r.action?.cluster || '-'}
									<tr class="hover:bg-gray-50">
										<td class="px-3 py-2">
											{#each verbs as verb}
												<span class="inline-block px-1.5 py-0.5 text-xs font-medium rounded {verb === 'GET' ? 'bg-green-100 text-green-700' : verb === 'POST' ? 'bg-blue-100 text-blue-700' : verb === 'PUT' ? 'bg-yellow-100 text-yellow-700' : verb === 'DELETE' ? 'bg-red-100 text-red-700' : verb === 'PATCH' ? 'bg-purple-100 text-purple-700' : 'bg-gray-100 text-gray-700'}">
													{verb}
												</span>
											{/each}
										</td>
										<td class="px-3 py-2">
											<span class="text-xs text-gray-500 font-mono">{pathType}</span>
										</td>
										<td class="px-3 py-2">
											<code class="text-sm text-gray-900 font-mono">{pathValue}</code>
										</td>
										<td class="px-3 py-2">
											<span class="text-sm text-purple-600 font-medium">{targetCluster}</span>
										</td>
										<td class="px-3 py-2">
											<span class="text-sm text-gray-500 font-mono">{getClusterEndpoint(targetCluster)}</span>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{:else}
					<p class="text-sm text-gray-500 italic">No routes defined</p>
				{/if}
			</div>
		{/each}

		{#if virtualHosts.length === 0}
			<p class="text-sm text-gray-500 italic">No domains configured</p>
		{/if}
	</div>
{/if}
