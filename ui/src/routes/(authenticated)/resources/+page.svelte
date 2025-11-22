<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount } from 'svelte';
	import ResourceSection from '$lib/components/ResourceSection.svelte';
	import RoutesTable from '$lib/components/RoutesTable.svelte';
	import type {
		ImportSummary,
		ListenerResponse,
		RouteResponse,
		ClusterResponse,
		SessionInfoResponse
	} from '$lib/api/types';

	let isLoading = $state(false);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let sessionInfo = $state<SessionInfoResponse | null>(null);

	// Data for each resource type
	let imports = $state<ImportSummary[]>([]);
	let listeners = $state<ListenerResponse[]>([]);
	let routes = $state<RouteResponse[]>([]);
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
			// Get first team to list imports (or use empty array if no teams)
			const team = sessionInfo?.teams[0] || '';

			// Load all resources in parallel
			const [importsData, listenersData, routesData, clustersData] = await Promise.all([
				team ? apiClient.listImports(team) : Promise.resolve([]),
				apiClient.listListeners(),
				apiClient.listRoutes(),
				apiClient.listClusters()
			]);

			imports = importsData;
			listeners = listenersData;
			routes = routesData;
			clusters = clustersData;
		} catch (err: any) {
			error = err.message || 'Failed to load resources';
		} finally {
			isLoading = false;
		}
	}

	// Filtered data based on search query only (backend handles team filtering)
	function getFilteredImports(): ImportSummary[] {
		if (!searchQuery) return imports;
		const query = searchQuery.toLowerCase();
		return imports.filter(
			(imp) =>
				imp.id.toLowerCase().includes(query) ||
				imp.specName.toLowerCase().includes(query) ||
				imp.team.toLowerCase().includes(query)
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

	function getFilteredRoutes(): RouteResponse[] {
		if (!searchQuery) return routes;
		const query = searchQuery.toLowerCase();
		return routes.filter(
			(route) =>
				route.name.toLowerCase().includes(query) ||
				route.pathPrefix.toLowerCase().includes(query) ||
				route.clusterTargets.toLowerCase().includes(query) ||
				route.team.toLowerCase().includes(query)
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
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
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

	function getImportSource(importId?: string): string {
		if (!importId) return 'Native';

		const importRecord = imports.find((imp) => imp.id === importId);
		if (importRecord) {
			return importRecord.specVersion
				? `${importRecord.specName} v${importRecord.specVersion}`
				: importRecord.specName;
		}

		return 'Unknown';
	}

	// Column definitions for each resource type
	const importsColumns = [
		{
			key: 'specName',
			label: 'Name',
			format: (value: any, row: ImportSummary) => ({
				type: 'link',
				text: value,
				href: `/imports/${row.id}`
			})
		},
		{
			key: 'specVersion',
			label: 'Version',
			format: (value: any) => value || 'N/A'
		},
		{
			key: 'team',
			label: 'Team',
			format: (value: any) => ({ type: 'badge', text: value, variant: 'blue' as const })
		},
		{ key: 'importedAt', label: 'Imported', format: (value: any) => formatDate(value) },
		{ key: 'updatedAt', label: 'Updated', format: (value: any) => formatDate(value) }
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
		{ key: 'version', label: 'Version', format: (value: any) => `v${value}` },
		{
			key: 'importId',
			label: 'Source',
			format: (value: any, row: ListenerResponse) => getImportSource(row.importId)
		}
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
		},
		{
			key: 'importId',
			label: 'Source',
			format: (value: any, row: ClusterResponse) => getImportSource(row.importId)
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
	<!-- Imports Section -->
	<ResourceSection
		title="OpenAPI Imports"
		count={getFilteredImports().length}
		columns={importsColumns}
		data={getFilteredImports()}
		emptyMessage="No imports found"
		actionButton={{ text: 'Import OpenAPI', href: '/imports/import' }}
	/>

	<!-- Listeners Section -->
	<ResourceSection
		title="Listeners"
		count={getFilteredListeners().length}
		columns={listenersColumns}
		data={getFilteredListeners()}
		emptyMessage="No listeners found"
	/>

	<!-- Routes Section -->
	<div class="mb-6">
		<RoutesTable
			routes={getFilteredRoutes()}
			{getImportSource}
			emptyMessage="No routes found"
		/>
	</div>

	<!-- Clusters Section -->
	<ResourceSection
		title="Clusters"
		count={getFilteredClusters().length}
		columns={clustersColumns}
		data={getFilteredClusters()}
		emptyMessage="No clusters found"
	/>
{/if}
