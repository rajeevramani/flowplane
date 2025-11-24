<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount } from 'svelte';
	import type { RouteResponse, ClusterResponse, ListenerResponse } from '$lib/api/types';

	interface VirtualHost {
		name?: string;
		domains?: string[];
		routes?: Array<{
			name?: string;
			match?: {
				path?: { type?: string; value?: string; Prefix?: string; Exact?: string };
				headers?: Array<{ name: string; value?: string }>;
			};
			action?: { type?: string; cluster?: string };
		}>;
	}

	interface ApiSummary {
		name: string;
		team: string;
		routeName: string;
		clusters: ClusterResponse[];
		listeners: ListenerResponse[];
		virtualHosts: VirtualHost[];
	}

	let apis = $state<ApiSummary[]>([]);
	let allClusters = $state<ClusterResponse[]>([]);
	let allListeners = $state<ListenerResponse[]>([]);
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let expandedApis = $state<Set<string>>(new Set());

	onMount(async () => {
		await loadApis();
	});

	async function loadApis() {
		isLoading = true;
		error = null;

		try {
			const [routes, clusters, listeners] = await Promise.all([
				apiClient.listRoutes(),
				apiClient.listClusters(),
				apiClient.listListeners()
			]);

			allClusters = clusters;
			allListeners = listeners;

			// Create API summary for each route
			const apiList: ApiSummary[] = routes.map((route) => {
				// Extract virtual hosts from route config
				const virtualHosts: VirtualHost[] =
					(route.config as { virtualHosts?: VirtualHost[] })?.virtualHosts || [];

				// Find clusters referenced in this route
				const clusterNames = new Set<string>();
				for (const vh of virtualHosts) {
					for (const r of vh.routes || []) {
						if (r.action?.cluster) {
							clusterNames.add(r.action.cluster);
						}
					}
				}
				const routeClusters = clusters.filter((c) => clusterNames.has(c.name));

				// Find listeners that reference this route
				const routeListeners = listeners.filter((l) => {
					const config = l.config as {
						filterChains?: Array<{
							filters?: Array<{ routeConfigName?: string }>;
						}>;
					};
					return config?.filterChains?.some((fc) =>
						fc.filters?.some((f) => f.routeConfigName === route.name)
					);
				});

				return {
					name: route.name.replace(/-routes$/, ''),
					team: route.team,
					routeName: route.name,
					clusters: routeClusters,
					listeners: routeListeners,
					virtualHosts
				};
			});

			apis = apiList;
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to load APIs';
		} finally {
			isLoading = false;
		}
	}

	function toggleExpand(apiName: string) {
		const newExpanded = new Set(expandedApis);
		if (newExpanded.has(apiName)) {
			newExpanded.delete(apiName);
		} else {
			newExpanded.add(apiName);
		}
		expandedApis = newExpanded;
	}

	function getEndpointsDisplay(cluster: ClusterResponse): string {
		try {
			const config = cluster.config as { endpoints?: Array<{ host?: string; port?: number }> };
			if (config?.endpoints && config.endpoints.length > 0) {
				return config.endpoints.map((e) => `${e.host || '?'}:${e.port || '?'}`).join(', ');
			}
		} catch {
			// ignore
		}
		return '-';
	}

	function getListenerAddress(listener: ListenerResponse): string {
		try {
			const config = listener.config as { address?: string; port?: number };
			return `${config?.address || '0.0.0.0'}:${config?.port || '?'}`;
		} catch {
			return '-';
		}
	}

	function getPathFromMatch(
		path: { type?: string; value?: string; Prefix?: string; Exact?: string } | undefined
	): string {
		if (!path) return '/';
		// Handle Rust enum format
		if (path.Prefix) return path.Prefix;
		if (path.Exact) return path.Exact;
		// Handle UI format
		return path.value || '/';
	}

	async function deleteApi(api: ApiSummary) {
		if (
			!confirm(
				`Are you sure you want to delete the API "${api.name}"? This will delete the route and may affect associated resources.`
			)
		) {
			return;
		}

		try {
			// Delete route
			await apiClient.deleteRoute(api.routeName);

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
			href="/apis/manage"
			class="inline-flex items-center gap-2 px-4 py-2 bg-blue-600 text-white font-medium rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
		>
			<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					stroke-width="2"
					d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"
				/>
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					stroke-width="2"
					d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"
				/>
			</svg>
			Manage Routes
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
			<svg
				class="mx-auto h-12 w-12 text-gray-400"
				fill="none"
				stroke="currentColor"
				viewBox="0 0 24 24"
			>
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					stroke-width="2"
					d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10"
				/>
			</svg>
			<h3 class="mt-4 text-lg font-medium text-gray-900">No APIs yet</h3>
			<p class="mt-2 text-gray-500">Get started by creating your first API.</p>
			<div class="mt-6">
				<a
					href="/apis/manage"
					class="inline-flex items-center gap-2 px-4 py-2 bg-blue-600 text-white font-medium rounded-md hover:bg-blue-700"
				>
					<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							stroke-width="2"
							d="M12 4v16m8-8H4"
						/>
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
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-8"
						></th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>Name</th
						>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>Team</th
						>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>Resources</th
						>
						<th
							class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
							>Actions</th
						>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each apis as api}
						<tr class="hover:bg-gray-50">
							<td class="px-6 py-4 whitespace-nowrap">
								<button
									onclick={() => toggleExpand(api.routeName)}
									class="text-gray-400 hover:text-gray-600 focus:outline-none"
									title={expandedApis.has(api.routeName) ? 'Collapse' : 'Expand'}
								>
									<svg
										class="h-5 w-5 transition-transform {expandedApis.has(api.routeName)
											? 'rotate-90'
											: ''}"
										fill="none"
										stroke="currentColor"
										viewBox="0 0 24 24"
									>
										<path
											stroke-linecap="round"
											stroke-linejoin="round"
											stroke-width="2"
											d="M9 5l7 7-7 7"
										/>
									</svg>
								</button>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<div class="text-sm font-medium text-gray-900">{api.name}</div>
								<div class="text-xs text-gray-500">{api.routeName}</div>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<span
									class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800"
								>
									{api.team}
								</span>
							</td>
							<td class="px-6 py-4 whitespace-nowrap">
								<div class="flex gap-1">
									<span
										class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-green-100 text-green-800"
										title="{api.clusters.length} cluster(s)"
									>
										C: {api.clusters.length}
									</span>
									<span
										class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-purple-100 text-purple-800"
										title="{api.virtualHosts.length} virtual host(s)"
									>
										VH: {api.virtualHosts.length}
									</span>
									<span
										class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-orange-100 text-orange-800"
										title="{api.listeners.length} listener(s)"
									>
										L: {api.listeners.length}
									</span>
								</div>
							</td>
							<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
								<button
									onclick={() => deleteApi(api)}
									class="text-red-600 hover:text-red-900"
									title="Delete API"
								>
									<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
										<path
											stroke-linecap="round"
											stroke-linejoin="round"
											stroke-width="2"
											d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"
										/>
									</svg>
								</button>
							</td>
						</tr>
						{#if expandedApis.has(api.routeName)}
							<tr class="bg-gray-50">
								<td colspan="5" class="px-6 py-4">
									<div class="space-y-4">
										<!-- Clusters -->
										<div>
											<h4 class="text-sm font-medium text-gray-700 mb-2">Clusters</h4>
											{#if api.clusters.length === 0}
												<p class="text-sm text-gray-500 italic">No clusters attached</p>
											{:else}
												<div class="space-y-2">
													{#each api.clusters as cluster}
														<div
															class="bg-white rounded border border-gray-200 p-3 text-sm"
														>
															<div class="flex items-center justify-between">
																<span class="font-medium text-gray-900"
																	>{cluster.name}</span
																>
																<span
																	class="text-xs px-2 py-0.5 bg-green-100 text-green-800 rounded"
																	>{cluster.team}</span
																>
															</div>
															<div class="mt-1 text-gray-500">
																Endpoints: {getEndpointsDisplay(cluster)}
															</div>
														</div>
													{/each}
												</div>
											{/if}
										</div>

										<!-- Virtual Hosts / Routes -->
										<div>
											<h4 class="text-sm font-medium text-gray-700 mb-2">
												Virtual Hosts & Routes
											</h4>
											{#if api.virtualHosts.length === 0}
												<p class="text-sm text-gray-500 italic">No virtual hosts configured</p>
											{:else}
												<div class="space-y-2">
													{#each api.virtualHosts as vh}
														<div class="bg-white rounded border border-gray-200 p-3 text-sm">
															<div class="font-medium text-gray-900">
																{vh.domains?.join(', ') || '*'}
															</div>
															{#if vh.routes && vh.routes.length > 0}
																<div class="mt-2 space-y-1">
																	{#each vh.routes as route}
																		<div
																			class="flex items-center gap-2 text-xs text-gray-600"
																		>
																			<code class="bg-gray-100 px-1.5 py-0.5 rounded">
																				{getPathFromMatch(route.match?.path)}
																			</code>
																			<span class="text-gray-400">-></span>
																			<span class="text-purple-600">
																				{route.action?.cluster || 'N/A'}
																			</span>
																		</div>
																	{/each}
																</div>
															{/if}
														</div>
													{/each}
												</div>
											{/if}
										</div>

										<!-- Listeners -->
										<div>
											<h4 class="text-sm font-medium text-gray-700 mb-2">Listeners</h4>
											{#if api.listeners.length === 0}
												<p class="text-sm text-gray-500 italic">No listeners attached</p>
											{:else}
												<div class="space-y-2">
													{#each api.listeners as listener}
														<div
															class="bg-white rounded border border-gray-200 p-3 text-sm"
														>
															<div class="flex items-center justify-between">
																<span class="font-medium text-gray-900"
																	>{listener.name}</span
																>
																<span
																	class="text-xs px-2 py-0.5 bg-orange-100 text-orange-800 rounded"
																	>{listener.team}</span
																>
															</div>
															<div class="mt-1 text-gray-500">
																Address: {getListenerAddress(listener)}
															</div>
														</div>
													{/each}
												</div>
											{/if}
										</div>
									</div>
								</td>
							</tr>
						{/if}
					{/each}
				</tbody>
			</table>
		</div>

		<div class="text-sm text-gray-500">
			<p><strong>Legend:</strong> C = Clusters, VH = Virtual Hosts, L = Listeners</p>
		</div>
	{/if}
</div>
