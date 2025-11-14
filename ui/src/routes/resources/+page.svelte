<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type {
		ApiDefinitionSummary,
		ListenerResponse,
		RouteResponse,
		ClusterResponse
	} from '$lib/api/types';

	type ResourceTab = 'api-definitions' | 'listeners' | 'routes' | 'clusters';

	let activeTab = $state<ResourceTab>('api-definitions');
	let isLoading = $state(false);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let teamFilter = $state('');

	// Data for each resource type
	let apiDefinitions = $state<ApiDefinitionSummary[]>([]);
	let listeners = $state<ListenerResponse[]>([]);
	let routes = $state<RouteResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);

	// Delete confirmation state
	let deleteConfirm = $state<{
		show: boolean;
		resourceType: string;
		resourceId: string;
		resourceName: string;
	} | null>(null);

	onMount(async () => {
		// Check authentication
		try {
			await apiClient.getSessionInfo();
			await loadResources();
		} catch (err) {
			goto('/login');
		}
	});

	async function loadResources() {
		isLoading = true;
		error = null;

		try {
			// Load all resources in parallel
			const [apiDefsData, listenersData, routesData, clustersData] = await Promise.all([
				apiClient.listApiDefinitions(teamFilter ? { team: teamFilter } : undefined),
				apiClient.listListeners(),
				apiClient.listRoutes(),
				apiClient.listClusters()
			]);

			apiDefinitions = apiDefsData;
			listeners = listenersData;
			routes = routesData;
			clusters = clustersData;
		} catch (err: any) {
			error = err.message || 'Failed to load resources';
		} finally {
			isLoading = false;
		}
	}

	function switchTab(tab: ResourceTab) {
		activeTab = tab;
	}

	function confirmDelete(resourceType: string, resourceId: string, resourceName: string) {
		deleteConfirm = {
			show: true,
			resourceType,
			resourceId,
			resourceName
		};
	}

	function cancelDelete() {
		deleteConfirm = null;
	}

	async function handleDelete() {
		if (!deleteConfirm) return;

		try {
			isLoading = true;

			switch (deleteConfirm.resourceType) {
				case 'api-definition':
					await apiClient.deleteApiDefinition(deleteConfirm.resourceId);
					break;
				case 'listener':
					await apiClient.deleteListener(deleteConfirm.resourceId);
					break;
				case 'route':
					await apiClient.deleteRoute(deleteConfirm.resourceId);
					break;
				case 'cluster':
					await apiClient.deleteCluster(deleteConfirm.resourceId);
					break;
			}

			// Reload resources
			await loadResources();
			deleteConfirm = null;
		} catch (err: any) {
			error = err.message || 'Failed to delete resource';
		} finally {
			isLoading = false;
		}
	}

	// Filtered data based on search query
	$effect(() => {
		// This will reactively update when searchQuery changes
		// For now, we'll do client-side filtering
		// Could be improved with server-side search
	});

	function getFilteredApiDefinitions(): ApiDefinitionSummary[] {
		if (!searchQuery) return apiDefinitions;
		const query = searchQuery.toLowerCase();
		return apiDefinitions.filter(
			(def) =>
				def.id.toLowerCase().includes(query) ||
				def.domain.toLowerCase().includes(query) ||
				def.team.toLowerCase().includes(query)
		);
	}

	function getFilteredListeners(): ListenerResponse[] {
		if (!searchQuery) return listeners;
		const query = searchQuery.toLowerCase();
		return listeners.filter(
			(listener) =>
				listener.name.toLowerCase().includes(query) ||
				listener.address.toLowerCase().includes(query)
		);
	}

	function getFilteredRoutes(): RouteResponse[] {
		if (!searchQuery) return routes;
		const query = searchQuery.toLowerCase();
		return routes.filter(
			(route) =>
				route.name.toLowerCase().includes(query) ||
				route.pathPrefix.toLowerCase().includes(query) ||
				route.clusterTargets.toLowerCase().includes(query)
		);
	}

	function getFilteredClusters(): ClusterResponse[] {
		if (!searchQuery) return clusters;
		const query = searchQuery.toLowerCase();
		return clusters.filter(
			(cluster) =>
				cluster.name.toLowerCase().includes(query) ||
				cluster.serviceName.toLowerCase().includes(query)
		);
	}

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}
</script>

<div class="min-h-screen bg-gray-50">
	<!-- Navigation -->
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a href="/dashboard" class="text-blue-600 hover:text-blue-800" aria-label="Back to dashboard">
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M10 19l-7-7m0 0l7-7m-7 7h18"
							/>
						</svg>
					</a>
					<h1 class="text-xl font-bold text-gray-900">Resource Management</h1>
				</div>
			</div>
		</div>
	</nav>

	<main class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
		<!-- Error Message -->
		{#if error}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{error}</p>
			</div>
		{/if}

		<!-- Search and Filters -->
		<div class="mb-6 flex gap-4">
			<div class="flex-1">
				<input
					type="text"
					bind:value={searchQuery}
					placeholder="Search resources..."
					class="w-full px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
			</div>
			<div>
				<input
					type="text"
					bind:value={teamFilter}
					onchange={() => loadResources()}
					placeholder="Filter by team..."
					class="px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
			</div>
		</div>

		<!-- Tabs -->
		<div class="bg-white rounded-t-lg shadow-md border-b border-gray-200">
			<div class="flex">
				<button
					onclick={() => switchTab('api-definitions')}
					class="flex-1 px-6 py-4 text-sm font-medium transition-colors {activeTab === 'api-definitions'
						? 'text-blue-600 border-b-2 border-blue-600'
						: 'text-gray-600 hover:text-gray-900'}"
				>
					API Definitions ({apiDefinitions.length})
				</button>
				<button
					onclick={() => switchTab('listeners')}
					class="flex-1 px-6 py-4 text-sm font-medium transition-colors {activeTab === 'listeners'
						? 'text-blue-600 border-b-2 border-blue-600'
						: 'text-gray-600 hover:text-gray-900'}"
				>
					Listeners ({listeners.length})
				</button>
				<button
					onclick={() => switchTab('routes')}
					class="flex-1 px-6 py-4 text-sm font-medium transition-colors {activeTab === 'routes'
						? 'text-blue-600 border-b-2 border-blue-600'
						: 'text-gray-600 hover:text-gray-900'}"
				>
					Routes ({routes.length})
				</button>
				<button
					onclick={() => switchTab('clusters')}
					class="flex-1 px-6 py-4 text-sm font-medium transition-colors {activeTab === 'clusters'
						? 'text-blue-600 border-b-2 border-blue-600'
						: 'text-gray-600 hover:text-gray-900'}"
				>
					Clusters ({clusters.length})
				</button>
			</div>
		</div>

		<!-- Tab Content -->
		<div class="bg-white rounded-b-lg shadow-md p-6">
			{#if isLoading}
				<div class="flex justify-center items-center py-12">
					<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
				</div>
			{:else if activeTab === 'api-definitions'}
				<!-- API Definitions - Card Layout -->
				{#if getFilteredApiDefinitions().length === 0}
					<p class="text-center text-gray-500 py-12">No API definitions found</p>
				{:else}
					<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
						{#each getFilteredApiDefinitions() as apiDef}
							<div class="border border-gray-200 rounded-lg p-6 hover:shadow-lg transition-shadow">
								<div class="flex justify-between items-start mb-4">
									<div>
										<h3 class="text-lg font-semibold text-gray-900">{apiDef.domain}</h3>
										<span class="inline-block mt-1 px-2 py-1 text-xs font-medium bg-blue-100 text-blue-800 rounded">
											{apiDef.team}
										</span>
									</div>
									<button
										onclick={() => confirmDelete('api-definition', apiDef.id, apiDef.domain)}
										class="text-red-600 hover:text-red-800"
										title="Delete"
									>
										<svg class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
											<path
												stroke-linecap="round"
												stroke-linejoin="round"
												stroke-width="2"
												d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"
											/>
										</svg>
									</button>
								</div>
								<div class="space-y-2 text-sm text-gray-600">
									<p><span class="font-medium">ID:</span> {apiDef.id}</p>
									<p><span class="font-medium">Version:</span> {apiDef.version}</p>
									<p>
										<span class="font-medium">Listener Isolation:</span>
										{apiDef.listenerIsolation ? 'Yes' : 'No'}
									</p>
									<p><span class="font-medium">Created:</span> {formatDate(apiDef.createdAt)}</p>
								</div>
<div class="mt-4 flex gap-2">
									<a
										href={`/api-definitions/${apiDef.id}`}
										class="flex-1 px-3 py-2 text-center text-sm font-medium text-white bg-blue-600 rounded hover:bg-blue-700"
									>
										View Details
									</a>
										<a
											href="/generate-envoy-config"
											class="px-3 py-2 text-sm font-medium text-blue-600 hover:text-blue-800 border border-blue-300 rounded hover:bg-blue-50"
											title="Generate Envoy Config"
										>
											Envoy Config
										</a>
								</div>
							</div>
						{/each}
					</div>
				{/if}
			{:else if activeTab === 'listeners'}
				<!-- Listeners - Table Layout -->
				{#if getFilteredListeners().length === 0}
					<p class="text-center text-gray-500 py-12">No listeners found</p>
				{:else}
					<div class="overflow-x-auto">
						<table class="min-w-full divide-y divide-gray-200">
							<thead class="bg-gray-50">
								<tr>
									<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
										Name
									</th>
									<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
										Address
									</th>
									<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
										Port
									</th>
									<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
										Protocol
									</th>
									<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
										Version
									</th>
									<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
										Actions
									</th>
								</tr>
							</thead>
							<tbody class="bg-white divide-y divide-gray-200">
								{#each getFilteredListeners() as listener}
									<tr class="hover:bg-gray-50">
										<td class="px-6 py-4 whitespace-nowrap text-sm font-medium text-gray-900">
											{listener.name}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
											{listener.address}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
											{listener.port || 'N/A'}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
											{listener.protocol}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
											{listener.version}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
											<button
												onclick={() => confirmDelete('listener', listener.name, listener.name)}
												class="text-red-600 hover:text-red-800"
											>
												Delete
											</button>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{/if}
			{:else if activeTab === 'routes'}
				<!-- Routes - Table Layout -->
				{#if getFilteredRoutes().length === 0}
					<p class="text-center text-gray-500 py-12">No routes found</p>
				{:else}
					<div class="overflow-x-auto">
						<table class="min-w-full divide-y divide-gray-200">
							<thead class="bg-gray-50">
								<tr>
									<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
										Name
									</th>
									<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
										Path Prefix
									</th>
									<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
										Cluster Targets
									</th>
									<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
										Actions
									</th>
								</tr>
							</thead>
							<tbody class="bg-white divide-y divide-gray-200">
								{#each getFilteredRoutes() as route}
									<tr class="hover:bg-gray-50">
										<td class="px-6 py-4 whitespace-nowrap text-sm font-medium text-gray-900">
											{route.name}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
											{route.pathPrefix}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
											{route.clusterTargets}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
											<button
												onclick={() => confirmDelete('route', route.name, route.name)}
												class="text-red-600 hover:text-red-800"
											>
												Delete
											</button>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{/if}
			{:else if activeTab === 'clusters'}
				<!-- Clusters - Table Layout -->
				{#if getFilteredClusters().length === 0}
					<p class="text-center text-gray-500 py-12">No clusters found</p>
				{:else}
					<div class="overflow-x-auto">
						<table class="min-w-full divide-y divide-gray-200">
							<thead class="bg-gray-50">
								<tr>
									<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
										Name
									</th>
									<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
										Service Name
									</th>
									<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
										Actions
									</th>
								</tr>
							</thead>
							<tbody class="bg-white divide-y divide-gray-200">
								{#each getFilteredClusters() as cluster}
									<tr class="hover:bg-gray-50">
										<td class="px-6 py-4 whitespace-nowrap text-sm font-medium text-gray-900">
											{cluster.name}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
											{cluster.serviceName}
										</td>
										<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
											<button
												onclick={() => confirmDelete('cluster', cluster.name, cluster.name)}
												class="text-red-600 hover:text-red-800"
											>
												Delete
											</button>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{/if}
			{/if}
		</div>
	</main>
</div>

<!-- Delete Confirmation Modal -->
{#if deleteConfirm}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
	>
		<div class="bg-white rounded-lg shadow-xl p-6 max-w-md w-full">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Confirm Delete</h2>
			<p class="text-sm text-gray-600 mb-6">
				Are you sure you want to delete the {deleteConfirm.resourceType}
				<strong class="text-gray-900">{deleteConfirm.resourceName}</strong>?
				This action cannot be undone.
			</p>
			<div class="flex justify-end gap-3">
				<button
					onclick={cancelDelete}
					class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200"
				>
					Cancel
				</button>
				<button
					onclick={handleDelete}
					class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700"
				>
					Delete
				</button>
			</div>
		</div>
	</div>
{/if}
