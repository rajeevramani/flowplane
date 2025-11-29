<script lang="ts">
	import type { RouteResponse } from '$lib/api/types';
	import Badge from './Badge.svelte';
	import RouteDetailRow, { type RouteDetail } from './RouteDetailRow.svelte';

	interface Props {
		routes: RouteResponse[];
		getImportSource: (importId: string | undefined) => string;
		emptyMessage?: string;
	}

	let { routes, getImportSource, emptyMessage = 'No routes found' }: Props = $props();

	let expandedRows = $state<Set<string>>(new Set());

	function toggleRow(name: string) {
		const newSet = new Set(expandedRows);
		if (newSet.has(name)) {
			newSet.delete(name);
		} else {
			newSet.add(name);
		}
		expandedRows = newSet;
	}

	function extractDomains(route: RouteResponse): string[] {
		const domains: string[] = [];
		const config = route.config as any;

		if (config?.virtualHosts) {
			for (const vh of config.virtualHosts) {
				if (vh.domains) {
					const nonWildcard = vh.domains.filter((d: string) => d !== '*');
					domains.push(...nonWildcard);
				}
			}
		}

		return [...new Set(domains)];
	}

	function countRoutes(route: RouteResponse): number {
		let count = 0;
		const config = route.config as any;

		if (config?.virtualHosts) {
			for (const vh of config.virtualHosts) {
				count += vh.routes?.length || 0;
			}
		}

		return count;
	}

	function extractRouteDetails(route: RouteResponse): RouteDetail[] {
		const details: RouteDetail[] = [];
		const config = route.config as any;

		if (config?.virtualHosts) {
			for (const vh of config.virtualHosts) {
				for (const r of vh.routes || []) {
					const methodHeader = r.match?.headers?.find(
						(h: any) => h.name === ':method'
					);
					const method = methodHeader?.value || '*';

					const pathMatch = r.match?.path;
					let path = '';
					let matchType: RouteDetail['matchType'] = 'exact';

					if (pathMatch) {
						matchType = pathMatch.type || 'exact';
						path = pathMatch.value || pathMatch.template || '';
					}

					// Handle Rust enum serialization: action is { "Cluster": {...} } or { "WeightedClusters": {...} }
					const clusterAction = r.action?.Cluster || r.action;
					const weightedAction = r.action?.WeightedClusters;

					// Extract cluster name
					let cluster = '';
					if (clusterAction?.name) {
						cluster = clusterAction.name;
					} else if (weightedAction?.clusters) {
						cluster = weightedAction.clusters.map((c: { name: string }) => c.name).join(', ');
					} else if (r.route?.cluster) {
						cluster = r.route.cluster;
					}

					// Extract rewrite info (from Cluster variant or flat structure)
					const prefixRewrite = clusterAction?.prefix_rewrite || clusterAction?.prefixRewrite || r.route?.prefixRewrite;
					const templateRewrite = clusterAction?.path_template_rewrite || clusterAction?.templateRewrite || r.route?.regexRewrite?.substitution;

					// Extract retry policy (from Cluster variant or flat structure)
					const rawRetryPolicy = clusterAction?.retry_policy || clusterAction?.retryPolicy || r.route?.retryPolicy;
					let retryPolicy: RouteDetail['retryPolicy'] | undefined;

					if (rawRetryPolicy) {
						// Handle retry_on as array (backend) or string (legacy)
						const retryOn = rawRetryPolicy.retry_on || rawRetryPolicy.retryOn;
						const retryOnStr = Array.isArray(retryOn) ? retryOn.join(', ') : retryOn;

						// Handle per_try_timeout_seconds (backend) or perTryTimeout (legacy)
						const perTryTimeout = rawRetryPolicy.per_try_timeout_seconds
							? `${rawRetryPolicy.per_try_timeout_seconds}s`
							: rawRetryPolicy.perTryTimeout;

						// Handle base_interval_ms/max_interval_ms (backend) or retryBackOff (legacy)
						let retryBackOff: RouteDetail['retryPolicy']['retryBackOff'] | undefined;
						if (rawRetryPolicy.base_interval_ms || rawRetryPolicy.max_interval_ms) {
							retryBackOff = {
								baseInterval: rawRetryPolicy.base_interval_ms ? `${rawRetryPolicy.base_interval_ms}ms` : undefined,
								maxInterval: rawRetryPolicy.max_interval_ms ? `${rawRetryPolicy.max_interval_ms}ms` : undefined
							};
						} else if (rawRetryPolicy.retryBackOff || rawRetryPolicy.retry_back_off) {
							retryBackOff = rawRetryPolicy.retryBackOff || rawRetryPolicy.retry_back_off;
						}

						retryPolicy = {
							numRetries: rawRetryPolicy.num_retries ?? rawRetryPolicy.numRetries,
							retryOn: retryOnStr,
							perTryTimeout,
							retryBackOff
						};
					}

					// Extract timeout (from Cluster variant or flat structure)
					const timeout = clusterAction?.timeout ?? clusterAction?.timeoutSeconds ?? r.route?.timeout;

					details.push({
						name: r.name,
						method,
						path,
						matchType,
						cluster,
						timeout,
						prefixRewrite,
						templateRewrite,
						retryPolicy
					});
				}
			}
		}

		return details;
	}

	function truncateCluster(cluster: string, maxLength: number = 30): string {
		if (cluster.length <= maxLength) return cluster;
		return cluster.substring(0, maxLength) + '...';
	}

	function formatDomains(domains: string[]): string {
		if (domains.length === 0) return 'N/A';
		if (domains.length === 1) return domains[0];
		return `${domains[0]} +${domains.length - 1}`;
	}
</script>

<div class="bg-white rounded-lg shadow">
	<div class="px-6 py-4 border-b border-gray-200">
		<h3 class="text-lg font-medium text-gray-900">
			Routes
			<span class="ml-2 text-sm font-normal text-gray-500">({routes.length})</span>
		</h3>
	</div>

	<div class="overflow-x-auto">
		{#if routes.length === 0}
			<p class="text-center text-gray-500 py-12">{emptyMessage}</p>
		{:else}
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-10"></th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Name</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Team</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Domains</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-20">Routes</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Cluster</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Source</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each routes as route (route.name)}
						{@const isExpanded = expandedRows.has(route.name)}
						{@const domains = extractDomains(route)}
						{@const routeCount = countRoutes(route)}
						{@const routeDetails = extractRouteDetails(route)}

						<tr
							class="hover:bg-gray-50 cursor-pointer transition-colors"
							onclick={() => toggleRow(route.name)}
						>
							<td class="px-4 py-4">
								<button
									class="text-gray-400 hover:text-gray-600 transition-transform duration-200"
									class:rotate-90={isExpanded}
									aria-label={isExpanded ? 'Collapse' : 'Expand'}
								>
									<svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
										<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
									</svg>
								</button>
							</td>
							<td class="px-4 py-4">
								<span class="text-sm font-medium text-gray-900">{route.name}</span>
							</td>
							<td class="px-4 py-4">
								<Badge variant="blue" size="sm">{route.team}</Badge>
							</td>
							<td class="px-4 py-4">
								<span class="text-sm text-gray-600" title={domains.join(', ')}>
									{formatDomains(domains)}
								</span>
							</td>
							<td class="px-4 py-4">
								<span class="text-sm font-medium text-gray-700">{routeCount}</span>
							</td>
							<td class="px-4 py-4">
								<span class="text-sm text-gray-600" title={route.clusterTargets}>
									{truncateCluster(route.clusterTargets)}
								</span>
							</td>
							<td class="px-4 py-4">
								<span class="text-sm text-gray-600">{getImportSource(route.importId)}</span>
							</td>
						</tr>

						{#if isExpanded && routeDetails.length > 0}
							<tr>
								<td colspan="7" class="p-0">
									<RouteDetailRow routes={routeDetails} />
								</td>
							</tr>
						{/if}
					{/each}
				</tbody>
			</table>
		{/if}
	</div>
</div>
