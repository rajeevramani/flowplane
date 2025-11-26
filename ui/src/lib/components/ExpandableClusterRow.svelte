<script lang="ts">
	import Badge from './Badge.svelte';
	import type { ClusterResponse, RouteResponse, ImportSummary } from '$lib/api/types';

	interface Props {
		cluster: ClusterResponse;
		routes?: RouteResponse[];
		imports?: ImportSummary[];
		onDelete?: (cluster: ClusterResponse) => void;
	}

	let { cluster, routes = [], imports = [], onDelete }: Props = $props();

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

	function getClusterConfig(): {
		endpoints: Array<{ host: string; port: number }>;
		lbPolicy?: string;
		connectTimeout?: number;
		healthChecks?: any[];
		circuitBreakers?: any;
		outlierDetection?: any;
	} {
		try {
			const config = cluster.config as any;
			return {
				endpoints: config?.endpoints || [],
				lbPolicy: config?.lbPolicy || config?.lb_policy,
				connectTimeout: config?.connectTimeoutSeconds || config?.connect_timeout_seconds,
				healthChecks: config?.healthChecks || config?.health_checks,
				circuitBreakers: config?.circuitBreakers || config?.circuit_breakers,
				outlierDetection: config?.outlierDetection || config?.outlier_detection
			};
		} catch {
			return { endpoints: [] };
		}
	}

	function getAssociatedRoutes(): RouteResponse[] {
		return routes.filter(route => {
			try {
				const config = route.config as any;
				const vhosts = config?.virtualHosts || [];
				for (const vh of vhosts) {
					for (const r of vh.routes || []) {
						if (r.action?.cluster === cluster.name) {
							return true;
						}
					}
				}
			} catch {
				// ignore
			}
			return false;
		});
	}

	function formatLbPolicy(policy?: string): string {
		if (!policy) return 'Round Robin';
		return policy.replace(/_/g, ' ').toLowerCase().replace(/\b\w/g, l => l.toUpperCase());
	}

	function handleDelete(e: Event) {
		e.stopPropagation();
		if (onDelete && confirm(`Delete cluster "${cluster.name}"?`)) {
			onDelete(cluster);
		}
	}

	const config = $derived(getClusterConfig());
	const associatedRoutes = $derived(getAssociatedRoutes());
	const primaryEndpoint = $derived(config.endpoints.length > 0 ? `${config.endpoints[0].host}:${config.endpoints[0].port}` : '-');
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

		<!-- Service Name -->
		<div class="w-48 min-w-0">
			<span class="font-medium text-gray-900 truncate block">{cluster.serviceName}</span>
		</div>

		<!-- Team -->
		<div class="w-24">
			<Badge variant="blue">{cluster.team}</Badge>
		</div>

		<!-- Primary Endpoint -->
		<div class="w-40 text-sm text-gray-600 font-mono truncate" title={primaryEndpoint}>
			{primaryEndpoint}
		</div>

		<!-- Endpoints count -->
		<div class="w-28 text-sm text-gray-600">
			{config.endpoints.length} endpoint{config.endpoints.length !== 1 ? 's' : ''}
		</div>

		<!-- LB Policy -->
		<div class="w-28 text-sm text-gray-600">
			{formatLbPolicy(config.lbPolicy)}
		</div>

		<!-- Source -->
		<div class="flex-1 text-sm text-gray-500 truncate">
			{getImportSource(cluster.importId)}
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
				<label class="block text-xs font-medium text-gray-500 uppercase">Cluster Name</label>
				<p class="mt-1 text-sm text-gray-900 font-mono">{cluster.name}</p>
			</div>
			<div>
				<label class="block text-xs font-medium text-gray-500 uppercase">Service Name</label>
				<p class="mt-1 text-sm text-gray-900">{cluster.serviceName}</p>
			</div>
			<div>
				<label class="block text-xs font-medium text-gray-500 uppercase">Load Balancing</label>
				<p class="mt-1 text-sm text-gray-900">{formatLbPolicy(config.lbPolicy)}</p>
			</div>
			<div>
				<label class="block text-xs font-medium text-gray-500 uppercase">Connect Timeout</label>
				<p class="mt-1 text-sm text-gray-900">{config.connectTimeout ? `${config.connectTimeout}s` : '-'}</p>
			</div>
		</div>

		<!-- Endpoints -->
		{#if config.endpoints.length > 0}
			<div class="mb-4">
				<h4 class="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">Endpoints</h4>
				<div class="bg-white rounded-lg border border-gray-200 overflow-hidden">
					<table class="min-w-full divide-y divide-gray-200">
						<thead class="bg-gray-50">
							<tr>
								<th class="px-3 py-2 text-left text-xs font-medium text-gray-500 uppercase">Host</th>
								<th class="px-3 py-2 text-left text-xs font-medium text-gray-500 uppercase">Port</th>
							</tr>
						</thead>
						<tbody class="divide-y divide-gray-100">
							{#each config.endpoints as ep}
								<tr class="hover:bg-gray-50">
									<td class="px-3 py-2 text-sm text-gray-900 font-mono">{ep.host}</td>
									<td class="px-3 py-2 text-sm text-gray-900 font-mono">{ep.port}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			</div>
		{/if}

		<!-- Features -->
		<div class="mb-4">
			<h4 class="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">Features</h4>
			<div class="flex flex-wrap gap-2">
				{#if config.healthChecks && config.healthChecks.length > 0}
					<span class="inline-flex items-center px-2.5 py-1 rounded-md text-xs font-medium bg-green-50 text-green-700 border border-green-200">
						Health Checks
					</span>
				{/if}
				{#if config.circuitBreakers}
					<span class="inline-flex items-center px-2.5 py-1 rounded-md text-xs font-medium bg-yellow-50 text-yellow-700 border border-yellow-200">
						Circuit Breakers
					</span>
				{/if}
				{#if config.outlierDetection}
					<span class="inline-flex items-center px-2.5 py-1 rounded-md text-xs font-medium bg-orange-50 text-orange-700 border border-orange-200">
						Outlier Detection
					</span>
				{/if}
				{#if !config.healthChecks?.length && !config.circuitBreakers && !config.outlierDetection}
					<span class="text-xs text-gray-400">No advanced features configured</span>
				{/if}
			</div>
		</div>

		<!-- Associated Routes -->
		{#if associatedRoutes.length > 0}
			<div>
				<h4 class="text-xs font-semibold text-gray-500 uppercase tracking-wide mb-2">Used By Routes</h4>
				<div class="flex flex-wrap gap-2">
					{#each associatedRoutes as route}
						<span class="inline-flex items-center gap-1.5 px-2.5 py-1 bg-purple-50 text-purple-700 text-xs rounded-md border border-purple-200">
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
