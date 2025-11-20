<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import Badge from '$lib/components/Badge.svelte';
	import type { ApiDefinitionSummary, ListenerResponse, ApiRouteResponse, ClusterResponse } from '$lib/api/types';

	let apiDefinition = $state<ApiDefinitionSummary | null>(null);
	let listeners = $state<ListenerResponse[]>([]);
	let routes = $state<ApiRouteResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let deleteConfirm = $state(false);
	let isDeleting = $state(false);

	// Get the ID from the URL
	const apiDefinitionId = $derived($page.params.id);

	onMount(async () => {
		await loadApiDefinition();
	});

	async function loadApiDefinition() {
		if (!apiDefinitionId) return;

		isLoading = true;
		error = null;

		try {
			// Load API definition and its routes directly from the API
			const [apiDef, apiRoutes, allListeners, allClusters] = await Promise.all([
				apiClient.getApiDefinition(apiDefinitionId),
				apiClient.getApiDefinitionRoutes(apiDefinitionId),
				apiClient.listListeners(),
				apiClient.listClusters()
			]);

			apiDefinition = apiDef;
			routes = apiRoutes;

			console.log('API Definition:', apiDef);
			console.log('API Routes:', apiRoutes);
			console.log('All Listeners:', allListeners);
			console.log('All Clusters:', allClusters);

			// Filter resources that belong to this API definition
			// Listeners and clusters are typically named with the API definition ID or domain
			const idPattern = apiDef.id;
			const domainPattern = apiDef.domain.replace(/\./g, '-');

			listeners = allListeners.filter(l =>
				l.team === apiDef.team &&
				(l.name.includes(idPattern) || l.name.includes(domainPattern) || l.name.includes(apiDef.domain))
			);

			clusters = allClusters.filter(c =>
				c.team === apiDef.team &&
				(c.name.includes(idPattern) || c.name.includes(domainPattern) || c.name.includes(apiDef.domain))
			);

			// If no resources found with strict filtering, fall back to team-only filtering
			if (listeners.length === 0) {
				listeners = allListeners.filter(l => l.team === apiDef.team);
			}
			if (clusters.length === 0) {
				clusters = allClusters.filter(c => c.team === apiDef.team);
			}
		} catch (err: any) {
			error = err.message || 'Failed to load API definition';
		} finally {
			isLoading = false;
		}
	}

	function confirmDelete() {
		deleteConfirm = true;
	}

	function cancelDelete() {
		deleteConfirm = false;
	}

	async function handleDelete() {
		if (!apiDefinition) return;

		try {
			isDeleting = true;
			await apiClient.deleteApiDefinition(apiDefinition.id);
			// Redirect to resources page after successful delete
			goto('/resources');
		} catch (err: any) {
			error = err.message || 'Failed to delete API definition';
			deleteConfirm = false;
		} finally {
			isDeleting = false;
		}
	}

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleString('en-US', {
			year: 'numeric',
			month: 'long',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function copyToClipboard(text: string) {
		navigator.clipboard.writeText(text);
	}

	// Helper to extract HTTP method from route headers
	function extractHttpMethod(route: ApiRouteResponse): string {
		if (!route.headers || !Array.isArray(route.headers)) {
			return 'ANY';
		}

		// Look for :method header in the headers array
		for (const header of route.headers) {
			if (header.name === ':method' || header.name === 'method') {
				return header.value || header.exactMatch || 'ANY';
			}
		}

		return 'ANY';
	}

	// Helper to extract cluster endpoints (host:port)
	function extractClusterEndpoint(cluster: ClusterResponse): { host: string; port: string } {
		try {
			if (cluster.config && typeof cluster.config === 'object') {
				const config = cluster.config as any;

				// Try config.endpoints first (Platform API format)
				if (config.endpoints && Array.isArray(config.endpoints) && config.endpoints.length > 0) {
					const endpoint = config.endpoints[0];
					return {
						host: endpoint.host || 'N/A',
						port: endpoint.port?.toString() || 'N/A'
					};
				}

				// Fallback: Try to find endpoints in config.load_assignment.endpoints (xDS format)
				const loadAssignment = config.load_assignment;
				if (loadAssignment && loadAssignment.endpoints && Array.isArray(loadAssignment.endpoints)) {
					const firstEndpoint = loadAssignment.endpoints[0];
					if (firstEndpoint && firstEndpoint.lb_endpoints && Array.isArray(firstEndpoint.lb_endpoints)) {
						const lbEndpoint = firstEndpoint.lb_endpoints[0];
						if (lbEndpoint && lbEndpoint.endpoint && lbEndpoint.endpoint.address) {
							const address = lbEndpoint.endpoint.address;
							if (address.socket_address) {
								return {
									host: address.socket_address.address || 'N/A',
									port: address.socket_address.port_value?.toString() || 'N/A'
								};
							}
						}
					}
				}
			}
		} catch (e) {
			console.error('Error extracting cluster endpoint:', e);
		}

		return { host: 'N/A', port: 'N/A' };
	}

	// Helper to format upstream targets
	function formatUpstreamTargets(upstreamTargets: any): string {
		if (!upstreamTargets) {
			return 'None';
		}

		// Handle both formats:
		// 1. Array of {host, port} objects
		// 2. Object with {targets: [{endpoint: "host:port"}]} structure
		if (Array.isArray(upstreamTargets)) {
			return upstreamTargets.map((target: any) => `${target.host}:${target.port}`).join(', ');
		}

		// Handle {targets: [{endpoint: "host:port", name: "..."}]} format
		if (upstreamTargets.targets && Array.isArray(upstreamTargets.targets)) {
			return upstreamTargets.targets.map((target: any) => target.endpoint).join(', ');
		}

		return 'None';
	}
</script>

<!-- Page Header with Actions -->
<div class="flex justify-between items-center mb-6">
	<div class="flex items-center gap-4">
		<a
			href="/resources"
			class="text-blue-600 hover:text-blue-800"
			aria-label="Back to resources"
		>
			<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					stroke-width="2"
					d="M10 19l-7-7m0 0l7-7m-7 7h18"
				/>
			</svg>
		</a>
		<h1 class="text-2xl font-bold text-gray-900">API Definition Details</h1>
	</div>
	{#if apiDefinition}
		<button
			onclick={confirmDelete}
			class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700"
		>
			Delete
		</button>
	{/if}
</div>

<!-- Error Message -->
{#if error}
	<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
		<p class="text-red-800 text-sm">{error}</p>
	</div>
{/if}

{#if isLoading}
	<div class="flex justify-center items-center py-12">
		<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
	</div>
{:else if apiDefinition}
	<!-- Header Section -->
	<div class="bg-white rounded-lg shadow-md p-6 mb-6">
		<div class="flex justify-between items-start">
			<div>
				<h2 class="text-2xl font-bold text-gray-900">{apiDefinition.domain}</h2>
				<div class="mt-2 flex items-center gap-3">
					<Badge variant="blue">Team: {apiDefinition.team}</Badge>
					<span class="text-sm text-gray-600">Version {apiDefinition.version}</span>
				</div>
			</div>
			<div class="text-right">
				<p class="text-sm text-gray-600">
					<span class="font-medium">ID:</span>
					{#if apiDefinition}
						<button
							onclick={() => copyToClipboard(apiDefinition?.id || '')}
							class="ml-1 text-blue-600 hover:text-blue-800 font-mono text-xs"
							title="Click to copy"
						>
							{apiDefinition.id}
						</button>
					{/if}
				</p>
			</div>
		</div>
	</div>

	<!-- Configuration Details -->
	<div class="grid grid-cols-1 lg:grid-cols-2 gap-6 mb-6">
		<!-- Basic Information -->
		<div class="bg-white rounded-lg shadow-md p-6">
			<h3 class="text-lg font-semibold text-gray-900 mb-4">Basic Information</h3>
			<dl class="space-y-3">
				<div>
					<dt class="text-sm font-medium text-gray-500">Domain</dt>
					<dd class="mt-1 text-sm text-gray-900">{apiDefinition.domain}</dd>
				</div>
				<div>
					<dt class="text-sm font-medium text-gray-500">Team</dt>
					<dd class="mt-1 text-sm text-gray-900">{apiDefinition.team}</dd>
				</div>
				<div>
					<dt class="text-sm font-medium text-gray-500">Version</dt>
					<dd class="mt-1 text-sm text-gray-900">{apiDefinition.version}</dd>
				</div>
				<div>
					<dt class="text-sm font-medium text-gray-500">Created At</dt>
					<dd class="mt-1 text-sm text-gray-900">{formatDate(apiDefinition.createdAt)}</dd>
				</div>
				<div>
					<dt class="text-sm font-medium text-gray-500">Updated At</dt>
					<dd class="mt-1 text-sm text-gray-900">{formatDate(apiDefinition.updatedAt)}</dd>
				</div>
			</dl>
		</div>

		<!-- Listener Configuration -->
		<div class="bg-white rounded-lg shadow-md p-6">
			<h3 class="text-lg font-semibold text-gray-900 mb-4">Listener Configuration</h3>
			<dl class="space-y-3">
				<div>
					<dt class="text-sm font-medium text-gray-500">Listener Isolation</dt>
					<dd class="mt-1">
						{#if apiDefinition.listenerIsolation}
							<span
								class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800"
							>
								Enabled
							</span>
							<p class="mt-1 text-sm text-gray-600">
								This API has a dedicated listener separate from other APIs.
							</p>
						{:else}
							<span
								class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-gray-100 text-gray-800"
							>
								Disabled
							</span>
							<p class="mt-1 text-sm text-gray-600">
								This API uses a shared listener with other APIs.
							</p>
						{/if}
					</dd>
				</div>
			</dl>
		</div>
	</div>

	<!-- Envoy Configuration -->
	{#if apiDefinition.bootstrapUri}
		<div class="bg-white rounded-lg shadow-md p-6 mb-6">
			<h3 class="text-lg font-semibold text-gray-900 mb-4">Envoy Configuration</h3>
			<div class="space-y-4">
				<a
					href="/generate-envoy-config"
					class="inline-flex items-center px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
				>
					Generate Bootstrap Configuration
				</a>
				<div class="bg-blue-50 border-l-4 border-blue-500 p-4">
					<div class="flex">
						<div class="flex-shrink-0">
							<svg
								class="h-5 w-5 text-blue-400"
								fill="currentColor"
								viewBox="0 0 20 20"
							>
								<path
									fill-rule="evenodd"
									d="M18 10a8 8 0 11-16 0 8 8 0 0116 0zm-7-4a1 1 0 11-2 0 1 1 0 012 0zM9 9a1 1 0 000 2v3a1 1 0 001 1h1a1 1 0 100-2v-3a1 1 0 00-1-1H9z"
									clip-rule="evenodd"
								/>
							</svg>
						</div>
						<div class="ml-3">
							<p class="text-sm text-blue-700">
								Generate the Envoy bootstrap configuration for this API definition. The configuration
								will include all routes, clusters, and listeners.
							</p>
						</div>
					</div>
				</div>
			</div>
		</div>
	{/if}

	<!-- Listeners Table -->
	<div class="bg-white rounded-lg shadow-md p-6 mb-6">
		<h3 class="text-lg font-semibold text-gray-900 mb-4">
			Listeners ({listeners.length})
		</h3>
		{#if listeners.length === 0}
			<p class="text-sm text-gray-500">No listeners found for this team</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-gray-200">
					<thead class="bg-gray-50">
						<tr>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								Name
							</th>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								Port
							</th>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								Protocol
							</th>
						</tr>
					</thead>
					<tbody class="bg-white divide-y divide-gray-200">
						{#each listeners as listener}
							<tr class="hover:bg-gray-50">
								<td class="px-6 py-4 whitespace-nowrap text-sm font-medium text-gray-900">
									{listener.name}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
									{listener.port || 'N/A'}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
									<Badge variant="blue">{listener.protocol}</Badge>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</div>

	<!-- Routes Table -->
	<div class="bg-white rounded-lg shadow-md p-6 mb-6">
		<h3 class="text-lg font-semibold text-gray-900 mb-4">
			Routes ({routes.length})
		</h3>
		{#if routes.length === 0}
			<p class="text-sm text-gray-500">No routes found for this API definition</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-gray-200">
					<thead class="bg-gray-50">
						<tr>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								Match Type
							</th>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								Path/Pattern
							</th>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								HTTP Method
							</th>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								Upstream Targets
							</th>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								Order
							</th>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								Timeout
							</th>
						</tr>
					</thead>
					<tbody class="bg-white divide-y divide-gray-200">
						{#each routes as route}
							<tr class="hover:bg-gray-50">
								<td class="px-6 py-4 whitespace-nowrap text-sm">
									<Badge variant="green">{route.matchType}</Badge>
								</td>
								<td class="px-6 py-4 text-sm text-gray-900 font-mono">
									{route.matchValue}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm">
									<Badge variant="purple">{extractHttpMethod(route)}</Badge>
								</td>
								<td class="px-6 py-4 text-sm text-gray-500">
									{formatUpstreamTargets(route.upstreamTargets)}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
									{route.routeOrder}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
									{route.timeoutSeconds ? `${route.timeoutSeconds}s` : 'Default'}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</div>

	<!-- Clusters Table -->
	<div class="bg-white rounded-lg shadow-md p-6 mb-6">
		<h3 class="text-lg font-semibold text-gray-900 mb-4">
			Clusters ({clusters.length})
		</h3>
		{#if clusters.length === 0}
			<p class="text-sm text-gray-500">No clusters found for this team</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-gray-200">
					<thead class="bg-gray-50">
						<tr>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								Service Name
							</th>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								Host
							</th>
							<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
								Port
							</th>
						</tr>
					</thead>
					<tbody class="bg-white divide-y divide-gray-200">
						{#each clusters as cluster}
							{@const endpoint = extractClusterEndpoint(cluster)}
							<tr class="hover:bg-gray-50">
								<td class="px-6 py-4 whitespace-nowrap text-sm font-medium text-gray-900">
									{cluster.serviceName}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500 font-mono">
									{endpoint.host}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
									{endpoint.port}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</div>

	<!-- Quick Actions -->
	<div class="bg-white rounded-lg shadow-md p-6">
		<h3 class="text-lg font-semibold text-gray-900 mb-4">Quick Actions</h3>
		<div class="flex flex-wrap gap-3">
			<a
				href="/resources"
				class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200"
			>
				← Back to All Resources
			</a>
			{#if apiDefinition.bootstrapUri}
				<a
					href="/generate-envoy-config"
					class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
				>
					Generate Envoy Config
				</a>
			{/if}
		</div>
	</div>
{/if}

<!-- Delete Confirmation Modal -->
{#if deleteConfirm && apiDefinition}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
	>
		<div class="bg-white rounded-lg shadow-xl p-6 max-w-md w-full">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Confirm Delete</h2>
			<p class="text-sm text-gray-600 mb-6">
				Are you sure you want to delete the API definition for
				<strong class="text-gray-900">{apiDefinition.domain}</strong>?
				This action cannot be undone and will remove all associated routes and configurations.
			</p>
			<div class="flex justify-end gap-3">
				<button
					onclick={cancelDelete}
					disabled={isDeleting}
					class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200 disabled:opacity-50"
				>
					Cancel
				</button>
				<button
					onclick={handleDelete}
					disabled={isDeleting}
					class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700 disabled:opacity-50"
				>
					{isDeleting ? 'Deleting...' : 'Delete'}
				</button>
			</div>
		</div>
	</div>
{/if}
