<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount } from 'svelte';
	import ResourceSection from '$lib/components/ResourceSection.svelte';
	import type {
		ApiDefinitionSummary,
		ListenerResponse,
		RouteResponse,
		ApiRouteResponse,
		ClusterResponse,
		SessionInfoResponse
	} from '$lib/api/types';

	let isLoading = $state(false);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let sessionInfo = $state<SessionInfoResponse | null>(null);

	// Data for each resource type
	let apiDefinitions = $state<ApiDefinitionSummary[]>([]);
	let listeners = $state<ListenerResponse[]>([]);
	let nativeRoutes = $state<RouteResponse[]>([]);
	let platformRoutes = $state<ApiRouteResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);

	onMount(async () => {
		try {
			sessionInfo = await apiClient.getSessionInfo();
			await loadResources();
		} catch (err: any) {
			error = err.message || 'Failed to load session info';
		}
	});

	async function loadResources() {
		isLoading = true;
		error = null;

		try {
			// Load all resources in parallel
			const [apiDefsData, listenersData, routesData, clustersData] = await Promise.all([
				apiClient.listApiDefinitions(),
				apiClient.listListeners(),
				apiClient.listRoutes(),
				apiClient.listClusters()
			]);

			apiDefinitions = apiDefsData;
			listeners = listenersData;
			nativeRoutes = routesData;
			clusters = clustersData;

			// Load platform routes from all API definitions
			const platformRoutesPromises = apiDefsData.map(apiDef =>
				apiClient.getApiDefinitionRoutes(apiDef.id).catch(() => [])
			);
			const platformRoutesArrays = await Promise.all(platformRoutesPromises);
			platformRoutes = platformRoutesArrays.flat();
		} catch (err: any) {
			error = err.message || 'Failed to load resources';
		} finally {
			isLoading = false;
		}
	}

	// Filtered data based on search query only (backend handles team filtering)
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
				listener.address.toLowerCase().includes(query) ||
				listener.team.toLowerCase().includes(query)
		);
	}

	function getFilteredNativeRoutes(): RouteResponse[] {
		if (!searchQuery) return nativeRoutes;
		const query = searchQuery.toLowerCase();
		return nativeRoutes.filter(
			(route) =>
				route.name.toLowerCase().includes(query) ||
				route.pathPrefix.toLowerCase().includes(query) ||
				route.clusterTargets.toLowerCase().includes(query) ||
				route.team.toLowerCase().includes(query)
		);
	}

	function getFilteredPlatformRoutes(): ApiRouteResponse[] {
		if (!searchQuery) return platformRoutes;
		const query = searchQuery.toLowerCase();
		return platformRoutes.filter(
			(route) =>
				route.matchValue.toLowerCase().includes(query) ||
				route.apiDefinitionId.toLowerCase().includes(query)
		);
	}

	function getFilteredClusters(): ClusterResponse[] {
		if (!searchQuery) return clusters;
		const query = searchQuery.toLowerCase();
		return clusters.filter(
			(cluster) =>
				cluster.name.toLowerCase().includes(query) ||
				cluster.serviceName.toLowerCase().includes(query) ||
				cluster.team.toLowerCase().includes(query)
		);
	}

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}

	function getApiDefinitionDomain(apiDefId: string): string {
		const apiDef = apiDefinitions.find(def => def.id === apiDefId);
		return apiDef?.domain || apiDefId;
	}

	function formatUpstreamTargets(upstreamTargets: any): string {
		if (!upstreamTargets) return 'None';

		if (Array.isArray(upstreamTargets)) {
			return upstreamTargets.map((target: any) => `${target.host}:${target.port}`).join(', ');
		}

		if (upstreamTargets.targets && Array.isArray(upstreamTargets.targets)) {
			return upstreamTargets.targets.map((target: any) => target.endpoint).join(', ');
		}

		return 'None';
	}

	function extractHttpMethod(route: ApiRouteResponse): string {
		if (!route.headers || !Array.isArray(route.headers)) {
			return 'ANY';
		}

		for (const header of route.headers) {
			if (header.name === ':method' || header.name === 'method') {
				return header.value || header.exactMatch || 'ANY';
			}
		}

		return 'ANY';
	}

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

	// Column definitions for each resource type
	const apiDefinitionsColumns = [
		{
			key: 'domain',
			label: 'Domain',
			format: (value: any, row: ApiDefinitionSummary) => ({
				type: 'link',
				text: value,
				href: `/api-definitions/${row.id}`
			})
		},
		{
			key: 'team',
			label: 'Team',
			format: (value: any) => ({ type: 'badge', text: value, variant: 'blue' as const })
		},
		{ key: 'id', label: 'ID' },
		{ key: 'version', label: 'Version', format: (value: any) => `v${value}` },
		{
			key: 'listenerIsolation',
			label: 'Listener Isolation',
			format: (value: any) => (value ? 'Yes' : 'No')
		},
		{ key: 'createdAt', label: 'Created', format: (value: any) => formatDate(value) }
	];

	const listenersColumns = [
		{ key: 'name', label: 'Name' },
		{
			key: 'team',
			label: 'Team',
			format: (value: any) => ({ type: 'badge', text: value, variant: 'blue' as const })
		},
		{ key: 'address', label: 'Address' },
		{ key: 'port', label: 'Port', format: (value: any) => value || 'N/A' },
		{ key: 'protocol', label: 'Protocol' },
		{ key: 'version', label: 'Version', format: (value: any) => `v${value}` }
	];

	const nativeRoutesColumns = [
		{ key: 'name', label: 'Name' },
		{
			key: 'team',
			label: 'Team',
			format: (value: any) => ({ type: 'badge', text: value, variant: 'blue' as const })
		},
		{ key: 'pathPrefix', label: 'Path Prefix' },
		{ key: 'clusterTargets', label: 'Cluster Targets' }
	];

	const platformRoutesColumns = [
		{
			key: 'matchType',
			label: 'Match Type',
			format: (value: any) => ({ type: 'badge', text: value, variant: 'green' as const })
		},
		{ key: 'matchValue', label: 'Path/Pattern' },
		{
			key: 'apiDefinitionId',
			label: 'API Definition',
			format: (value: any) => ({
				type: 'link',
				text: getApiDefinitionDomain(value),
				href: `/api-definitions/${value}`
			})
		},
		{
			key: 'headers',
			label: 'HTTP Method',
			format: (value: any, row: ApiRouteResponse) => ({
				type: 'badge',
				text: extractHttpMethod(row),
				variant: 'purple' as const
			})
		},
		{
			key: 'upstreamTargets',
			label: 'Upstream Targets',
			format: (value: any) => formatUpstreamTargets(value)
		},
		{ key: 'routeOrder', label: 'Order' }
	];

	const clustersColumns = [
		{ key: 'serviceName', label: 'Service Name' },
		{
			key: 'team',
			label: 'Team',
			format: (value: any) => ({ type: 'badge', text: value, variant: 'blue' as const })
		},
		{
			key: 'config',
			label: 'Host',
			format: (value: any, row: ClusterResponse) => extractClusterEndpoint(row).host
		},
		{
			key: 'config',
			label: 'Port',
			format: (value: any, row: ClusterResponse) => extractClusterEndpoint(row).port
		}
	];
</script>

<!-- Page Header -->
<div class="mb-6">
	<h1 class="text-2xl font-bold text-gray-900">Resource Management</h1>
	<p class="mt-1 text-sm text-gray-600">
		View and manage all accessible resources
	</p>
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
		placeholder="Search across all resources..."
		class="w-full px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
	/>
</div>

{#if isLoading}
	<div class="flex justify-center items-center py-12">
		<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
	</div>
{:else}
	<!-- API Definitions Section -->
	<ResourceSection
		title="API Definitions"
		count={getFilteredApiDefinitions().length}
		columns={apiDefinitionsColumns}
		data={getFilteredApiDefinitions()}
		emptyMessage="No API definitions found"
		actionButton={{ text: 'Import OpenAPI', href: '/api-definitions/import' }}
	/>

	<!-- Listeners Section -->
	<ResourceSection
		title="Listeners"
		count={getFilteredListeners().length}
		columns={listenersColumns}
		data={getFilteredListeners()}
		emptyMessage="No listeners found"
	/>

	<!-- Platform API Routes Section -->
	<ResourceSection
		title="Platform API Routes"
		count={getFilteredPlatformRoutes().length}
		columns={platformRoutesColumns}
		data={getFilteredPlatformRoutes()}
		emptyMessage="No platform API routes found"
	/>

	<!-- Native API Routes Section -->
	<ResourceSection
		title="Native API Routes"
		count={getFilteredNativeRoutes().length}
		columns={nativeRoutesColumns}
		data={getFilteredNativeRoutes()}
		emptyMessage="No native API routes found"
	/>

	<!-- Clusters Section -->
	<ResourceSection
		title="Clusters"
		count={getFilteredClusters().length}
		columns={clustersColumns}
		data={getFilteredClusters()}
		emptyMessage="No clusters found"
	/>
{/if}
