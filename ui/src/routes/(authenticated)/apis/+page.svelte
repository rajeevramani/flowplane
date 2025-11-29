<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import { Plus, Eye } from 'lucide-svelte';
	import DetailDrawer from '$lib/components/DetailDrawer.svelte';
	import ConfigCard from '$lib/components/ConfigCard.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import Tooltip from '$lib/components/Tooltip.svelte';
	import type { RouteResponse, ClusterResponse, ListenerResponse, ImportSummary } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import type { Unsubscriber } from 'svelte/store';

	interface RouteDetail {
		name: string;
		apiName: string;
		team: string;
		method: string;
		path: string;
		matchType: 'exact' | 'prefix' | 'template' | 'regex';
		cluster: string;
		timeout?: number;
		prefixRewrite?: string;
		templateRewrite?: string;
		retryPolicy?: {
			numRetries?: number;
			retryOn?: string;
			perTryTimeout?: string;
			retryBackOff?: {
				baseInterval?: string;
				maxInterval?: string;
			};
		};
		sourceRoute: RouteResponse;
	}

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');
	let unsubscribe: Unsubscriber;

	// Data
	let routes = $state<RouteResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);
	let listeners = $state<ListenerResponse[]>([]);
	let imports = $state<ImportSummary[]>([]);

	// Drawer state
	let drawerOpen = $state(false);
	let selectedRoute = $state<RouteResponse | null>(null);

	onMount(async () => {
		unsubscribe = selectedTeam.subscribe(async (team) => {
			if (team && team !== currentTeam) {
				currentTeam = team;
				await loadData();
			}
		});
	});

	onDestroy(() => {
		if (unsubscribe) unsubscribe();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		try {
			const [routesData, clustersData, listenersData, importsData] = await Promise.all([
				apiClient.listRoutes(),
				apiClient.listClusters(),
				apiClient.listListeners(),
				currentTeam ? apiClient.listImports(currentTeam) : Promise.resolve([])
			]);

			routes = routesData;
			clusters = clustersData;
			listeners = listenersData;
			imports = importsData;

			// Console log the route data for debugging
			console.log('Routes data:', routesData);
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Extract and flatten all route details from all routes
	function extractAllRouteDetails(routes: RouteResponse[]): RouteDetail[] {
		const allDetails: RouteDetail[] = [];

		for (const route of routes) {
			const config = route.config;

			if (config?.virtualHosts) {
				for (const vh of config.virtualHosts) {
					for (const r of vh.routes || []) {
						const methodHeader = r.match?.headers?.find((h: { name: string }) => h.name === ':method');
						const method = methodHeader?.value || '*';

						const pathMatch = r.match?.path;
						let path = '';
						let matchType: RouteDetail['matchType'] = 'exact';

						if (pathMatch) {
							matchType = pathMatch.type || 'exact';
							path = pathMatch.value || pathMatch.template || '';
						}

						// Handle Rust enum serialization
						const clusterAction = r.action?.Cluster || r.action;
						const weightedAction = r.action?.WeightedClusters;

						// Extract cluster name (handle both 'name' and 'cluster' field names)
						let cluster = '';
						if (clusterAction?.name) {
							cluster = clusterAction.name;
						} else if (clusterAction?.cluster) {
							cluster = clusterAction.cluster;
						} else if (weightedAction?.clusters) {
							cluster = weightedAction.clusters.map((c: { name: string }) => c.name).join(', ');
						} else if (r.route?.cluster) {
							cluster = r.route.cluster;
						}

						// Extract rewrite info
						const prefixRewrite = clusterAction?.prefix_rewrite || clusterAction?.prefixRewrite || r.route?.prefixRewrite;
						const templateRewrite = clusterAction?.path_template_rewrite || clusterAction?.templateRewrite || r.route?.regexRewrite?.substitution;

						// Extract retry policy
						const rawRetryPolicy = clusterAction?.retry_policy || clusterAction?.retryPolicy || r.route?.retryPolicy;
						let retryPolicy: RouteDetail['retryPolicy'] | undefined;

						if (rawRetryPolicy) {
							const retryOn = rawRetryPolicy.retry_on || rawRetryPolicy.retryOn;
							const retryOnStr = Array.isArray(retryOn) ? retryOn.join(', ') : retryOn;

							// Handle perTryTimeout from multiple possible field names
							let perTryTimeout: string | undefined;
							if (rawRetryPolicy.per_try_timeout_seconds) {
								perTryTimeout = `${rawRetryPolicy.per_try_timeout_seconds}s`;
							} else if (rawRetryPolicy.perTryTimeoutSeconds) {
								perTryTimeout = `${rawRetryPolicy.perTryTimeoutSeconds}s`;
							} else if (rawRetryPolicy.perTryTimeout) {
								perTryTimeout = rawRetryPolicy.perTryTimeout;
							}

							// Handle backoff from multiple possible structures
							let retryBackOff: RouteDetail['retryPolicy']['retryBackOff'] | undefined;
							if (rawRetryPolicy.base_interval_ms || rawRetryPolicy.max_interval_ms) {
								retryBackOff = {
									baseInterval: rawRetryPolicy.base_interval_ms ? `${rawRetryPolicy.base_interval_ms}ms` : undefined,
									maxInterval: rawRetryPolicy.max_interval_ms ? `${rawRetryPolicy.max_interval_ms}ms` : undefined
								};
							} else if (rawRetryPolicy.backoff) {
								// Handle camelCase backoff object from API
								retryBackOff = {
									baseInterval: rawRetryPolicy.backoff.baseIntervalMs ? `${rawRetryPolicy.backoff.baseIntervalMs}ms` : undefined,
									maxInterval: rawRetryPolicy.backoff.maxIntervalMs ? `${rawRetryPolicy.backoff.maxIntervalMs}ms` : undefined
								};
							} else if (rawRetryPolicy.retryBackOff || rawRetryPolicy.retry_back_off) {
								retryBackOff = rawRetryPolicy.retryBackOff || rawRetryPolicy.retry_back_off;
							}

							retryPolicy = {
								numRetries: rawRetryPolicy.num_retries ?? rawRetryPolicy.numRetries ?? rawRetryPolicy.maxRetries,
								retryOn: retryOnStr,
								perTryTimeout,
								retryBackOff
							};
						}

						// Extract timeout
						const timeout = clusterAction?.timeout ?? clusterAction?.timeoutSeconds ?? r.route?.timeout;

						allDetails.push({
							name: r.name,
							apiName: route.name,
							team: route.team,
							method,
							path,
							matchType,
							cluster,
							timeout,
							prefixRewrite,
							templateRewrite,
							retryPolicy,
							sourceRoute: route
						});
					}
				}
			}
		}

		return allDetails;
	}

	// Filter routes by team and search, then flatten
	let filteredRouteDetails = $derived(() => {
		const teamFiltered = routes.filter((route) => {
			if (currentTeam && route.team !== currentTeam) return false;
			return true;
		});

		const allDetails = extractAllRouteDetails(teamFiltered);

		if (!searchQuery) return allDetails;

		const query = searchQuery.toLowerCase();
		return allDetails.filter((detail) =>
			detail.apiName.toLowerCase().includes(query) ||
			detail.team.toLowerCase().includes(query) ||
			detail.path.toLowerCase().includes(query) ||
			detail.cluster.toLowerCase().includes(query)
		);
	});

	function getClusterNamesForRoute(route: RouteResponse): Set<string> {
		const clusterNames = new Set<string>();
		route.config?.virtualHosts?.forEach((vh: { routes?: unknown[] }) => {
			vh.routes?.forEach((r: unknown) => {
				const route = r as { action?: { Cluster?: { name?: string }, WeightedClusters?: { clusters?: { name: string }[] }, cluster?: string }, route?: { cluster?: string } };
				const clusterAction = route.action?.Cluster;
				const weightedAction = route.action?.WeightedClusters;

				if (clusterAction?.name) {
					clusterNames.add(clusterAction.name);
				} else if (weightedAction?.clusters) {
					weightedAction.clusters.forEach((c) => clusterNames.add(c.name));
				} else if (route.action?.cluster) {
					clusterNames.add(route.action.cluster);
				} else if (route.route?.cluster) {
					clusterNames.add(route.route.cluster);
				}
			});
		});
		return clusterNames;
	}

	function getMethodBadgeVariant(method: string): 'green' | 'blue' | 'yellow' | 'red' | 'gray' {
		switch (method.toUpperCase()) {
			case 'GET': return 'green';
			case 'POST': return 'blue';
			case 'PUT':
			case 'PATCH': return 'yellow';
			case 'DELETE': return 'red';
			default: return 'gray';
		}
	}

	function getMatchTypeLabel(matchType: string): string {
		switch (matchType) {
			case 'exact': return 'Exact';
			case 'prefix': return 'Prefix';
			case 'template': return 'Template';
			case 'regex': return 'Regex';
			default: return matchType;
		}
	}

	function truncateText(text: string, maxLength: number = 25): string {
		if (text.length <= maxLength) return text;
		return text.substring(0, maxLength) + '...';
	}

	function formatRewrite(detail: RouteDetail): string | null {
		if (detail.prefixRewrite) return detail.prefixRewrite;
		if (detail.templateRewrite) return detail.templateRewrite;
		return null;
	}

	function formatRetryOn(retryOn: string | undefined): string {
		if (!retryOn) return '-';
		return retryOn.split(',').map((s) => s.trim()).join(', ');
	}

	function formatBackoff(policy: RouteDetail['retryPolicy']): string {
		if (!policy?.retryBackOff) return '-';
		const base = policy.retryBackOff.baseInterval || '25ms';
		const max = policy.retryBackOff.maxInterval || '250ms';
		return `${base} - ${max}`;
	}

	function openDrawer(route: RouteResponse) {
		selectedRoute = route;
		drawerOpen = true;
	}

	function closeDrawer() {
		drawerOpen = false;
		selectedRoute = null;
	}

	async function handleDelete(routeName: string) {
		if (!confirm(`Are you sure you want to delete the API "${routeName}"?`)) return;

		try {
			await apiClient.deleteRoute(routeName);
			await loadData();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete route';
		}
	}

	function getListenerForRoute(route: RouteResponse): ListenerResponse | undefined {
		return listeners.find((l) =>
			l.config?.filterChains?.some((fc: { filters?: { routeConfigName?: string }[] }) =>
				fc.filters?.some((f) => f.routeConfigName === route.name)
			)
		);
	}

	function getRetryPoliciesForRoute(route: RouteResponse): Array<{ routeName: string; policy: RouteDetail['retryPolicy'] }> {
		const policies: Array<{ routeName: string; policy: RouteDetail['retryPolicy'] }> = [];

		if (route.config?.virtualHosts) {
			for (const vh of route.config.virtualHosts) {
				for (const r of vh.routes || []) {
					const clusterAction = r.action?.Cluster || r.action;
					const rawRetryPolicy = clusterAction?.retry_policy || clusterAction?.retryPolicy;

					if (rawRetryPolicy) {
						const retryOn = rawRetryPolicy.retry_on || rawRetryPolicy.retryOn;
						const retryOnStr = Array.isArray(retryOn) ? retryOn.join(', ') : retryOn;

						let perTryTimeout: string | undefined;
						if (rawRetryPolicy.per_try_timeout_seconds) {
							perTryTimeout = `${rawRetryPolicy.per_try_timeout_seconds}s`;
						} else if (rawRetryPolicy.perTryTimeoutSeconds) {
							perTryTimeout = `${rawRetryPolicy.perTryTimeoutSeconds}s`;
						} else if (rawRetryPolicy.perTryTimeout) {
							perTryTimeout = rawRetryPolicy.perTryTimeout;
						}

						let retryBackOff: RouteDetail['retryPolicy']['retryBackOff'] | undefined;
						if (rawRetryPolicy.backoff) {
							retryBackOff = {
								baseInterval: rawRetryPolicy.backoff.baseIntervalMs ? `${rawRetryPolicy.backoff.baseIntervalMs}ms` : undefined,
								maxInterval: rawRetryPolicy.backoff.maxIntervalMs ? `${rawRetryPolicy.backoff.maxIntervalMs}ms` : undefined
							};
						} else if (rawRetryPolicy.base_interval_ms || rawRetryPolicy.max_interval_ms) {
							retryBackOff = {
								baseInterval: rawRetryPolicy.base_interval_ms ? `${rawRetryPolicy.base_interval_ms}ms` : undefined,
								maxInterval: rawRetryPolicy.max_interval_ms ? `${rawRetryPolicy.max_interval_ms}ms` : undefined
							};
						}

						policies.push({
							routeName: r.name || 'unnamed',
							policy: {
								numRetries: rawRetryPolicy.num_retries ?? rawRetryPolicy.numRetries ?? rawRetryPolicy.maxRetries,
								retryOn: retryOnStr,
								perTryTimeout,
								retryBackOff
							}
						});
					}
				}
			}
		}

		return policies;
	}

	function getClustersForRoute(route: RouteResponse): ClusterResponse[] {
		const clusterNames = getClusterNamesForRoute(route);
		return clusters.filter((c) => clusterNames.has(c.name));
	}

	function getUniqueRowId(detail: RouteDetail, index: number): string {
		return `${detail.apiName}-${detail.name || index}-${detail.method}-${detail.path}`;
	}
</script>

<!-- Page Header -->
<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="text-2xl font-bold text-gray-900">APIs</h1>
		<p class="mt-1 text-sm text-gray-600">Manage your API routes and configurations</p>
	</div>
	<div class="flex gap-3">
		<a
			href="/apis/manage"
			class="inline-flex items-center gap-2 px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-md hover:bg-blue-700 transition-colors"
		>
			<Plus class="h-4 w-4" />
			Manage APIs
		</a>
		<a
			href="/imports/import"
			class="inline-flex items-center gap-2 px-4 py-2 bg-white text-gray-700 text-sm font-medium rounded-md border border-gray-300 hover:bg-gray-50 transition-colors"
		>
			Import OpenAPI
		</a>
	</div>
</div>

<!-- Error Message -->
{#if error}
	<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
		<p class="text-red-800 text-sm">{error}</p>
	</div>
{/if}

<!-- Search Bar -->
<div class="mb-6">
	<input
		type="text"
		bind:value={searchQuery}
		placeholder="Search APIs..."
		class="w-full max-w-md px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
	/>
</div>

<!-- APIs Table -->
<div class="bg-white rounded-lg shadow">
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
		</div>
	{:else if filteredRouteDetails().length === 0}
		<div class="text-center py-12 text-gray-500">
			No APIs found. Create one to get started.
		</div>
	{:else}
		<div class="overflow-x-auto">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Name</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Team</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-20">Method</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Path</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-20">Match</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Cluster</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Rewrite</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-24">Retries</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-20">Timeout</th>
						<th class="px-4 py-3 text-center text-xs font-medium text-gray-500 uppercase tracking-wider w-16">Actions</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each filteredRouteDetails() as detail, index (getUniqueRowId(detail, index))}
						{@const rewrite = formatRewrite(detail)}
						{@const rowId = getUniqueRowId(detail, index)}

						<tr class="hover:bg-gray-50 transition-colors">
							<td class="px-4 py-3">
								<button
									onclick={() => openDrawer(detail.sourceRoute)}
									class="font-medium text-blue-600 hover:text-blue-800 hover:underline text-left"
								>
									{truncateText(detail.apiName, 20)}
								</button>
							</td>
							<td class="px-4 py-3">
								<Badge variant="indigo" size="sm">{detail.team}</Badge>
							</td>
							<td class="px-4 py-3">
								<Badge variant={getMethodBadgeVariant(detail.method)} size="sm">
									{detail.method.toUpperCase()}
								</Badge>
							</td>
							<td class="px-4 py-3">
								<code class="text-sm text-gray-800 bg-gray-100 px-2 py-0.5 rounded font-mono" title={detail.path}>
									{truncateText(detail.path || '/', 30)}
								</code>
							</td>
							<td class="px-4 py-3">
								<span class="text-xs bg-blue-100 text-blue-700 px-2 py-0.5 rounded font-medium">
									{getMatchTypeLabel(detail.matchType)}
								</span>
							</td>
							<td class="px-4 py-3">
								<span class="text-sm text-gray-600" title={detail.cluster}>
									{truncateText(detail.cluster, 25)}
								</span>
							</td>
							<td class="px-4 py-3">
								{#if rewrite}
									<span class="text-sm text-gray-600 flex items-center gap-1">
										<span class="text-gray-400">&rarr;</span>
										<code class="font-mono text-xs bg-gray-100 px-1.5 py-0.5 rounded" title={rewrite}>
											{truncateText(rewrite, 20)}
										</code>
									</span>
								{:else}
									<span class="text-gray-400">-</span>
								{/if}
							</td>
							<td class="px-4 py-3">
								{#if detail.retryPolicy && detail.retryPolicy.numRetries}
									<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-orange-100 text-orange-800">
										{detail.retryPolicy.numRetries}
									</span>
								{:else}
									<span class="text-gray-400">-</span>
								{/if}
							</td>
							<td class="px-4 py-3 text-center">
								<span class="text-sm text-gray-500">
									{detail.timeout ? `${detail.timeout}s` : '-'}
								</span>
							</td>
							<td class="px-4 py-3 text-center">
								<button
									onclick={(e) => {
										e.stopPropagation();
										openDrawer(detail.sourceRoute);
									}}
									class="p-1.5 rounded hover:bg-gray-100 text-gray-500 hover:text-blue-600 transition-colors"
									title="View details"
								>
									<Eye class="h-4 w-4" />
								</button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>

<!-- Detail Drawer -->
<DetailDrawer
	open={drawerOpen}
	title={selectedRoute?.name || ''}
	subtitle={selectedRoute ? `Team: ${selectedRoute.team}` : undefined}
	onClose={closeDrawer}
>
	{#if selectedRoute}
		<div class="space-y-6">
			<!-- Overview -->
			<ConfigCard title="Overview" variant="gray">
				<dl class="grid grid-cols-2 gap-4 text-sm">
					<div>
						<dt class="text-gray-500">Path Prefix</dt>
						<dd class="font-mono text-gray-900">{selectedRoute.pathPrefix}</dd>
					</div>
					<div>
						<dt class="text-gray-500">Cluster Targets</dt>
						<dd class="text-gray-900">{selectedRoute.clusterTargets}</dd>
					</div>
					{#if selectedRoute.importId}
						{@const importRecord = imports.find((i) => i.id === selectedRoute?.importId)}
						<div>
							<dt class="text-gray-500">Source</dt>
							<dd class="text-gray-900">
								{importRecord?.specName || 'Imported'}
								{#if importRecord?.specVersion}
									<span class="text-gray-500">v{importRecord.specVersion}</span>
								{/if}
							</dd>
						</div>
					{/if}
				</dl>
			</ConfigCard>

			<!-- Listener -->
			{#if getListenerForRoute(selectedRoute)}
				{@const listener = getListenerForRoute(selectedRoute)}
				<ConfigCard title="Listener" variant="blue">
					<dl class="grid grid-cols-2 gap-4 text-sm">
						<div>
							<dt class="text-gray-500">Name</dt>
							<dd class="text-gray-900">{listener?.name}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Address</dt>
							<dd class="font-mono text-gray-900">{listener?.address}:{listener?.port}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Protocol</dt>
							<dd class="text-gray-900">{listener?.protocol}</dd>
						</div>
					</dl>
				</ConfigCard>
			{/if}

			<!-- Clusters -->
			{#if getClustersForRoute(selectedRoute).length > 0}
				{@const routeClusters = getClustersForRoute(selectedRoute)}
				<ConfigCard title="Clusters" variant="green">
					<div class="space-y-3">
						{#each routeClusters as cluster}
							<div class="p-3 bg-white rounded border border-green-200">
								<div class="font-medium text-gray-900">{cluster.serviceName}</div>
								<div class="text-sm text-gray-500">{cluster.name}</div>
							</div>
						{/each}
					</div>
				</ConfigCard>
			{/if}

			<!-- Retry Policies -->
			{#if getRetryPoliciesForRoute(selectedRoute).length > 0}
				{@const retryPolicies = getRetryPoliciesForRoute(selectedRoute)}
				<ConfigCard title="Retry Policies" variant="orange">
					<div class="space-y-3">
						{#each retryPolicies as { routeName, policy }}
							<div class="p-3 bg-white rounded border border-orange-200">
								<div class="font-medium text-gray-900 text-sm mb-2">{routeName}</div>
								<dl class="grid grid-cols-2 gap-2 text-sm">
									<div>
										<dt class="text-gray-500">Max Retries</dt>
										<dd class="font-medium text-gray-900">{policy.numRetries}</dd>
									</div>
									<div>
										<dt class="text-gray-500">Per Try Timeout</dt>
										<dd class="font-medium text-gray-900">{policy.perTryTimeout || '-'}</dd>
									</div>
									<div class="col-span-2">
										<dt class="text-gray-500">Retry On</dt>
										<dd class="font-medium text-gray-900">{formatRetryOn(policy.retryOn)}</dd>
									</div>
									{#if policy.retryBackOff}
										<div class="col-span-2">
											<dt class="text-gray-500">Backoff</dt>
											<dd class="font-medium text-gray-900">{formatBackoff(policy)}</dd>
										</div>
									{/if}
								</dl>
							</div>
						{/each}
					</div>
				</ConfigCard>
			{/if}

			<!-- Domains -->
			{#if selectedRoute.config?.virtualHosts}
				<ConfigCard title="Domains" variant="gray" collapsible defaultCollapsed>
					<div class="space-y-4">
						{#each selectedRoute.config.virtualHosts as vh}
							<div class="p-3 bg-white rounded border border-gray-200">
								<div class="font-medium text-gray-900">{vh.name}</div>
								<div class="text-sm text-gray-500 mt-1">
									{vh.domains?.join(', ') || '*'}
								</div>
								{#if vh.routes}
									<div class="mt-2 text-sm text-gray-600">
										{vh.routes.length} route{vh.routes.length !== 1 ? 's' : ''}
									</div>
								{/if}
							</div>
						{/each}
					</div>
				</ConfigCard>
			{/if}
		</div>
	{/if}

	{#snippet footer()}
		<div class="flex justify-end gap-3">
			<Button variant="ghost" onclick={closeDrawer}>Close</Button>
			<Button variant="danger" onclick={() => selectedRoute && handleDelete(selectedRoute.name)}>
				Delete
			</Button>
		</div>
	{/snippet}
</DetailDrawer>
