<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount } from 'svelte';
	import type { RouteResponse, ClusterResponse, ListenerResponse } from '$lib/api/types';

	interface ApiSummary {
		name: string;
		team: string;
		clusterName: string | null;
		routeName: string | null;
		listenerName: string | null;
		endpoints: string;
		pathPrefix: string;
		domain: string;
	}

	let apis = $state<ApiSummary[]>([]);
	let isLoading = $state(true);
	let error = $state<string | null>(null);

	onMount(async () => {
		await loadApis();
	});

	async function loadApis() {
		isLoading = true;
		error = null;

		try {
			// Load all resources
			const [routes, clusters, listeners] = await Promise.all([
				apiClient.listRoutes(),
				apiClient.listClusters(),
				apiClient.listListeners()
			]);

			// Group routes by naming pattern to identify "APIs"
			// An "API" is identified by routes ending in "-routes"
			const apiMap = new Map<string, ApiSummary>();

			for (const route of routes) {
				// Extract base name from route name (e.g., "payment-service-routes" -> "payment-service")
				const baseName = route.name.replace(/-routes$/, '');
				if (baseName === route.name) continue; // Not following our naming convention

				// Find matching cluster
				const clusterName = `${baseName}-cluster`;
				const cluster = clusters.find((c) => c.name === clusterName);

				// Find matching listener
				const listenerName = `${baseName}-listener`;
				const listener = listeners.find((l) => l.name === listenerName);

				// Extract info from route config
				let pathPrefix = '/';
				let domain = '*';
				try {
					if (route.config && typeof route.config === 'object') {
						const config = route.config as {
							virtualHosts?: Array<{
								domains?: string[];
								routes?: Array<{ match?: { path?: { value?: string } } }>;
							}>;
						};
						if (config.virtualHosts && config.virtualHosts[0]) {
							const vhost = config.virtualHosts[0];
							if (vhost.domains && vhost.domains[0]) {
								domain = vhost.domains[0];
							}
							if (vhost.routes && vhost.routes[0]?.match?.path?.value) {
								pathPrefix = vhost.routes[0].match.path.value;
							}
						}
					}
				} catch {
					// Keep defaults
				}

				// Extract endpoints from cluster
				let endpoints = '-';
				try {
					if (cluster?.config && typeof cluster.config === 'object') {
						const config = cluster.config as {
							endpoints?: Array<{ host?: string; port?: number }>;
						};
						if (config.endpoints && config.endpoints.length > 0) {
							endpoints = config.endpoints
								.map((e) => `${e.host || '?'}:${e.port || '?'}`)
								.join(', ');
						}
					}
				} catch {
					// Keep default
				}

				apiMap.set(baseName, {
					name: baseName,
					team: route.team,
					clusterName: cluster ? clusterName : null,
					routeName: route.name,
					listenerName: listener ? listenerName : null,
					endpoints,
					pathPrefix,
					domain
				});
			}

			apis = Array.from(apiMap.values());
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to load APIs';
		} finally {
			isLoading = false;
		}
	}

	async function deleteApi(api: ApiSummary) {
		if (!confirm(`Are you sure you want to delete the API "${api.name}"? This will delete the associated cluster, route, and listener.`)) {
			return;
		}

		try {
			// Delete in reverse order: listener, route, cluster
			if (api.listenerName) {
				await apiClient.deleteListener(api.listenerName);
			}
			if (api.routeName) {
				await apiClient.deleteRoute(api.routeName);
			}
			if (api.clusterName) {
				await apiClient.deleteCluster(api.clusterName);
			}

			// Reload the list
			await loadApis();
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to delete API';
		}
	}
</script>

<div class="space-y-6">
	<div class="flex items-center justify-between">
		<h1 class="text-2xl font-bold text-gray-900">APIs</h1>
		<a
			href="/apis/create"
			class="inline-flex items-center gap-2 px-4 py-2 bg-blue-600 text-white font-medium rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
		>
			<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
			</svg>
			Create API
		</a>
	</div>

	{#if error}
		<div class="bg-red-50 border-l-4 border-red-500 p-4 rounded-md">
			<p class="text-red-700">{error}</p>
		</div>
	{/if}

	{#if isLoading}
		<div class="flex justify-center py-12">
			<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
		</div>
	{:else if apis.length === 0}
		<div class="bg-white rounded-lg shadow-md p-12 text-center">
			<svg class="mx-auto h-12 w-12 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10" />
			</svg>
			<h3 class="mt-4 text-lg font-medium text-gray-900">No APIs yet</h3>
			<p class="mt-2 text-gray-500">Get started by creating your first API.</p>
			<div class="mt-6">
				<a
					href="/apis/create"
					class="inline-flex items-center gap-2 px-4 py-2 bg-blue-600 text-white font-medium rounded-md hover:bg-blue-700"
				>
					<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
						<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
					</svg>
					Create API
				</a>
			</div>
		</div>
	{:else}
		<div class="bg-white rounded-lg shadow-md overflow-hidden">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Name</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Team</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Endpoints</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Path</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Domain</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Resources</th>
						<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">Actions</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each apis as api}
						<tr class="hover:bg-gray-50">
							<td class="px-6 py-4 whitespace-nowrap">
								<div class="text-sm font-medium text-gray-900">{api.name}</div>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800">
									{api.team}
								</span>
							</td>
							<td class="px-6 py-4">
								<div class="text-sm text-gray-500 max-w-xs truncate" title={api.endpoints}>{api.endpoints}</div>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<code class="text-sm text-gray-600 bg-gray-100 px-2 py-0.5 rounded">{api.pathPrefix}</code>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<span class="text-sm text-gray-500">{api.domain}</span>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<div class="flex gap-1">
									{#if api.clusterName}
										<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-green-100 text-green-800" title={api.clusterName}>C</span>
									{/if}
									{#if api.routeName}
										<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-purple-100 text-purple-800" title={api.routeName}>R</span>
									{/if}
									{#if api.listenerName}
										<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-orange-100 text-orange-800" title={api.listenerName}>L</span>
									{/if}
								</div>
							</td>
							<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
								<button
									onclick={() => deleteApi(api)}
									class="text-red-600 hover:text-red-900"
									title="Delete API"
								>
									<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
										<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
									</svg>
								</button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		<div class="text-sm text-gray-500">
			<p><strong>Legend:</strong> C = Cluster, R = Route, L = Listener</p>
		</div>
	{/if}
</div>
