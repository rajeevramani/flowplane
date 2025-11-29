<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import { Eye, Trash2, MoreVertical } from 'lucide-svelte';
	import DataTable from '$lib/components/DataTable.svelte';
	import FeatureBadges from '$lib/components/FeatureBadges.svelte';
	import StatusIndicator from '$lib/components/StatusIndicator.svelte';
	import DetailDrawer from '$lib/components/DetailDrawer.svelte';
	import ConfigCard from '$lib/components/ConfigCard.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import type { ClusterResponse, RouteResponse, ImportSummary } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import type { Unsubscriber } from 'svelte/store';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');
	let unsubscribe: Unsubscriber;

	// Data
	let clusters = $state<ClusterResponse[]>([]);
	let routes = $state<RouteResponse[]>([]);
	let imports = $state<ImportSummary[]>([]);

	// Drawer state
	let drawerOpen = $state(false);
	let selectedCluster = $state<ClusterResponse | null>(null);

	// Action menu state
	let openMenuId = $state<string | null>(null);

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
			const [clustersData, routesData, importsData] = await Promise.all([
				apiClient.listClusters(),
				apiClient.listRoutes(),
				currentTeam ? apiClient.listImports(currentTeam) : Promise.resolve([])
			]);

			clusters = clustersData;
			routes = routesData;
			imports = importsData;
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Table columns
	const columns = [
		{ key: 'serviceName', label: 'Service', sortable: true },
		{ key: 'team', label: 'Team', sortable: true },
		{ key: 'endpoints', label: 'Endpoints' },
		{ key: 'features', label: 'Features' },
		{ key: 'lbPolicy', label: 'LB Policy' },
		{ key: 'status', label: 'Status' },
		{ key: 'source', label: 'Source' }
	];

	// Transform clusters for table display
	let tableData = $derived(
		clusters
			.filter((cluster) => {
				// Filter by team if a team is selected
				if (currentTeam && cluster.team !== currentTeam) return false;

				// Filter by search query
				if (!searchQuery) return true;
				const query = searchQuery.toLowerCase();
				return (
					cluster.name.toLowerCase().includes(query) ||
					cluster.serviceName.toLowerCase().includes(query) ||
					cluster.team.toLowerCase().includes(query)
				);
			})
			.map((cluster) => {
				const config = cluster.config || {};
				const endpointCount = config.loadAssignment?.endpoints?.[0]?.lbEndpoints?.length || 0;
				const importRecord = cluster.importId
					? imports.find((i) => i.id === cluster.importId)
					: null;

				return {
					id: cluster.name,
					name: cluster.name,
					serviceName: cluster.serviceName,
					team: cluster.team,
					endpoints: endpointCount,
					lbPolicy: config.lbPolicy || 'ROUND_ROBIN',
					hasRetry: false,
					hasCircuitBreaker: Boolean(config.circuitBreakers),
					hasOutlierDetection: Boolean(config.outlierDetection),
					hasHealthCheck: Boolean(config.healthChecks?.length),
					source: importRecord ? importRecord.specName : 'Manual',
					sourceType: importRecord ? 'import' : 'manual',
					_raw: cluster
				};
			})
	);

	function openDrawer(row: Record<string, unknown>) {
		selectedCluster = (row._raw as ClusterResponse) || null;
		drawerOpen = true;
	}

	function closeDrawer() {
		drawerOpen = false;
		selectedCluster = null;
	}

	async function handleDelete(clusterName: string) {
		if (!confirm(`Are you sure you want to delete the cluster "${clusterName}"?`)) return;

		try {
			await apiClient.deleteCluster(clusterName);
			await loadData();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete cluster';
		}
		openMenuId = null;
	}

	function toggleMenu(e: MouseEvent, id: string) {
		e.stopPropagation();
		openMenuId = openMenuId === id ? null : id;
	}

	function closeAllMenus() {
		openMenuId = null;
	}

	// Get routes using this cluster
	function getRoutesForCluster(cluster: ClusterResponse): RouteResponse[] {
		return routes.filter((route) => {
			return route.config?.virtualHosts?.some((vh: { routes?: { route?: { cluster?: string } }[] }) =>
				vh.routes?.some((r) => r.route?.cluster === cluster.name)
			);
		});
	}

	// Format LB policy for display
	function formatLbPolicy(policy: string): string {
		return policy.replace(/_/g, ' ').toLowerCase().replace(/\b\w/g, (c) => c.toUpperCase());
	}
</script>

<svelte:window onclick={closeAllMenus} />

<!-- Page Header -->
<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="text-2xl font-bold text-gray-900">Clusters</h1>
		<p class="mt-1 text-sm text-gray-600">Backend services and upstream endpoints</p>
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
		placeholder="Search clusters..."
		class="w-full max-w-md px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
	/>
</div>

<!-- Data Table -->
<DataTable
	{columns}
	data={tableData}
	loading={isLoading}
	emptyMessage="No clusters found."
	rowKey="id"
	onRowClick={openDrawer}
>
	{#snippet cell({ row, column })}
		{#if column.key === 'serviceName'}
			<span class="font-medium text-blue-600 hover:text-blue-800">{row.serviceName}</span>
		{:else if column.key === 'team'}
			<Badge variant="indigo">{row.team}</Badge>
		{:else if column.key === 'endpoints'}
			<span class="text-gray-600">{row.endpoints} endpoint{row.endpoints !== 1 ? 's' : ''}</span>
		{:else if column.key === 'features'}
			<FeatureBadges
				hasRetry={row.hasRetry as boolean}
				hasCircuitBreaker={row.hasCircuitBreaker as boolean}
				hasOutlierDetection={row.hasOutlierDetection as boolean}
				hasHealthCheck={row.hasHealthCheck as boolean}
			/>
		{:else if column.key === 'lbPolicy'}
			<span class="text-gray-600">{formatLbPolicy(row.lbPolicy as string)}</span>
		{:else if column.key === 'status'}
			<StatusIndicator status="active" />
		{:else if column.key === 'source'}
			{#if row.sourceType === 'import'}
				<Badge variant="purple">{row.source}</Badge>
			{:else}
				<span class="text-gray-500">{row.source}</span>
			{/if}
		{:else}
			{String(row[column.key] ?? '')}
		{/if}
	{/snippet}

	{#snippet actions({ row })}
		<div class="relative">
			<button
				onclick={(e) => toggleMenu(e, row.id as string)}
				class="p-1 rounded hover:bg-gray-100"
			>
				<MoreVertical class="h-4 w-4 text-gray-500" />
			</button>

			{#if openMenuId === row.id}
				<div
					class="absolute right-0 mt-1 w-36 bg-white rounded-md shadow-lg border border-gray-200 z-10"
					onclick={(e) => e.stopPropagation()}
				>
					<button
						onclick={() => openDrawer(row)}
						class="flex items-center gap-2 w-full px-3 py-2 text-sm text-gray-700 hover:bg-gray-100"
					>
						<Eye class="h-4 w-4" />
						View Details
					</button>
					<button
						onclick={() => handleDelete(row.name as string)}
						class="flex items-center gap-2 w-full px-3 py-2 text-sm text-red-600 hover:bg-red-50"
					>
						<Trash2 class="h-4 w-4" />
						Delete
					</button>
				</div>
			{/if}
		</div>
	{/snippet}
</DataTable>

<!-- Detail Drawer -->
<DetailDrawer
	open={drawerOpen}
	title={selectedCluster?.serviceName || ''}
	subtitle={selectedCluster ? `Team: ${selectedCluster.team}` : undefined}
	onClose={closeDrawer}
>
	{#if selectedCluster}
		{@const config = selectedCluster.config || {}}
		<div class="space-y-6">
			<!-- Overview -->
			<ConfigCard title="Overview" variant="gray">
				<dl class="grid grid-cols-2 gap-4 text-sm">
					<div>
						<dt class="text-gray-500">Cluster Name</dt>
						<dd class="font-mono text-gray-900">{selectedCluster.name}</dd>
					</div>
					<div>
						<dt class="text-gray-500">LB Policy</dt>
						<dd class="text-gray-900">{formatLbPolicy(config.lbPolicy || 'ROUND_ROBIN')}</dd>
					</div>
					<div>
						<dt class="text-gray-500">Connect Timeout</dt>
						<dd class="text-gray-900">{config.connectTimeout || '5s'}</dd>
					</div>
					<div>
						<dt class="text-gray-500">DNS Lookup Family</dt>
						<dd class="text-gray-900">{config.dnsLookupFamily || 'AUTO'}</dd>
					</div>
				</dl>
			</ConfigCard>

			<!-- Endpoints -->
			{#if (config.loadAssignment?.endpoints?.[0]?.lbEndpoints || []).length > 0}
				{@const endpoints = config.loadAssignment?.endpoints?.[0]?.lbEndpoints || []}
				<ConfigCard title="Endpoints" variant="green">
					<div class="space-y-2">
						{#each endpoints as ep}
							{@const addr = ep.endpoint?.address?.socketAddress}
							{#if addr}
								<div class="flex items-center justify-between p-2 bg-white rounded border border-green-200">
									<span class="font-mono text-gray-900">{addr.address}:{addr.portValue}</span>
									<StatusIndicator status="active" showLabel={false} />
								</div>
							{/if}
						{/each}
					</div>
				</ConfigCard>
			{/if}

			<!-- Circuit Breaker -->
			{#if config.circuitBreakers}
				<ConfigCard title="Circuit Breaker" variant="yellow">
					{@const thresholds = config.circuitBreakers.thresholds?.[0] || {}}
					<dl class="grid grid-cols-2 gap-4 text-sm">
						<div>
							<dt class="text-gray-500">Max Connections</dt>
							<dd class="text-gray-900">{thresholds.maxConnections || 1024}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Max Pending Requests</dt>
							<dd class="text-gray-900">{thresholds.maxPendingRequests || 1024}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Max Requests</dt>
							<dd class="text-gray-900">{thresholds.maxRequests || 1024}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Max Retries</dt>
							<dd class="text-gray-900">{thresholds.maxRetries || 3}</dd>
						</div>
					</dl>
				</ConfigCard>
			{/if}

			<!-- Outlier Detection -->
			{#if config.outlierDetection}
				<ConfigCard title="Outlier Detection" variant="orange">
					<dl class="grid grid-cols-2 gap-4 text-sm">
						<div>
							<dt class="text-gray-500">Consecutive 5xx</dt>
							<dd class="text-gray-900">{config.outlierDetection.consecutive5xx || 5}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Interval</dt>
							<dd class="text-gray-900">{config.outlierDetection.interval || '10s'}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Base Ejection Time</dt>
							<dd class="text-gray-900">{config.outlierDetection.baseEjectionTime || '30s'}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Max Ejection %</dt>
							<dd class="text-gray-900">{config.outlierDetection.maxEjectionPercent || 10}%</dd>
						</div>
					</dl>
				</ConfigCard>
			{/if}

			<!-- Health Checks -->
			{#if config.healthChecks?.length}
				<ConfigCard title="Health Checks" variant="green">
					{#each config.healthChecks as hc}
						<dl class="grid grid-cols-2 gap-4 text-sm">
							<div>
								<dt class="text-gray-500">Type</dt>
								<dd class="text-gray-900">{hc.httpHealthCheck ? 'HTTP' : 'TCP'}</dd>
							</div>
							{#if hc.httpHealthCheck?.path}
								<div>
									<dt class="text-gray-500">Path</dt>
									<dd class="font-mono text-gray-900">{hc.httpHealthCheck.path}</dd>
								</div>
							{/if}
							<div>
								<dt class="text-gray-500">Interval</dt>
								<dd class="text-gray-900">{hc.interval || '5s'}</dd>
							</div>
							<div>
								<dt class="text-gray-500">Timeout</dt>
								<dd class="text-gray-900">{hc.timeout || '5s'}</dd>
							</div>
						</dl>
					{/each}
				</ConfigCard>
			{/if}

			<!-- Associated Routes -->
			{#if getRoutesForCluster(selectedCluster).length > 0}
				{@const clusterRoutes = getRoutesForCluster(selectedCluster)}
				<ConfigCard title="Associated Routes" variant="blue" collapsible defaultCollapsed>
					<div class="space-y-2">
						{#each clusterRoutes as route}
							<div class="p-2 bg-white rounded border border-blue-200">
								<div class="font-medium text-gray-900">{route.name}</div>
								<div class="text-sm text-gray-500">{route.pathPrefix}</div>
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
			<Button variant="danger" onclick={() => selectedCluster && handleDelete(selectedCluster.name)}>
				Delete
			</Button>
		</div>
	{/snippet}
</DetailDrawer>
