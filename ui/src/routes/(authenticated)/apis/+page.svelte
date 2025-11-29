<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import { Plus, Eye, Trash2, MoreVertical } from 'lucide-svelte';
	import DataTable from '$lib/components/DataTable.svelte';
	import FeatureBadges from '$lib/components/FeatureBadges.svelte';
	import StatusIndicator from '$lib/components/StatusIndicator.svelte';
	import DetailDrawer from '$lib/components/DetailDrawer.svelte';
	import ConfigCard from '$lib/components/ConfigCard.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import type { RouteResponse, ClusterResponse, ListenerResponse, ImportSummary } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import type { Unsubscriber } from 'svelte/store';

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
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Table columns
	const columns = [
		{ key: 'name', label: 'Name', sortable: true },
		{ key: 'team', label: 'Team', sortable: true },
		{ key: 'routes', label: 'Routes' },
		{ key: 'features', label: 'Features' },
		{ key: 'status', label: 'Status' },
		{ key: 'source', label: 'Source' }
	];

	// Transform routes for table display
	let tableData = $derived(
		routes
			.filter((route) => {
				if (!searchQuery) return true;
				const query = searchQuery.toLowerCase();
				return (
					route.name.toLowerCase().includes(query) ||
					route.team.toLowerCase().includes(query) ||
					route.pathPrefix.toLowerCase().includes(query)
				);
			})
			.map((route) => {
				// Count routes from virtual hosts
				const routeCount = route.config?.virtualHosts?.reduce(
					(sum: number, vh: { routes?: unknown[] }) => sum + (vh.routes?.length || 0),
					0
				) || 1;

				// Get source info
				const importRecord = route.importId
					? imports.find((i) => i.id === route.importId)
					: null;

				return {
					id: route.name,
					name: route.name,
					team: route.team,
					routes: routeCount,
					hasRetry: hasRetryPolicy(route),
					hasCircuitBreaker: false,
					hasOutlierDetection: false,
					hasHealthCheck: false,
					source: importRecord ? importRecord.specName : 'Manual',
					sourceType: importRecord ? 'import' : 'manual',
					_raw: route
				};
			})
	);

	function hasRetryPolicy(route: RouteResponse): boolean {
		const config = route.config;
		if (!config?.virtualHosts) return false;

		return config.virtualHosts.some((vh: { routes?: { route?: { retryPolicy?: unknown } }[] }) =>
			vh.routes?.some((r) => r.route?.retryPolicy)
		);
	}

	function openDrawer(row: Record<string, unknown>) {
		selectedRoute = (row._raw as RouteResponse) || null;
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
		openMenuId = null;
	}

	function toggleMenu(e: MouseEvent, id: string) {
		e.stopPropagation();
		openMenuId = openMenuId === id ? null : id;
	}

	function closeAllMenus() {
		openMenuId = null;
	}

	// Get listener for route
	function getListenerForRoute(route: RouteResponse): ListenerResponse | undefined {
		return listeners.find((l) =>
			l.config?.filterChains?.some((fc: { filters?: { routeConfigName?: string }[] }) =>
				fc.filters?.some((f) => f.routeConfigName === route.name)
			)
		);
	}

	// Get clusters used by route
	function getClustersForRoute(route: RouteResponse): ClusterResponse[] {
		const clusterNames = new Set<string>();

		route.config?.virtualHosts?.forEach((vh: { routes?: { route?: { cluster?: string } }[] }) => {
			vh.routes?.forEach((r) => {
				if (r.route?.cluster) {
					clusterNames.add(r.route.cluster);
				}
			});
		});

		return clusters.filter((c) => clusterNames.has(c.name));
	}
</script>

<svelte:window onclick={closeAllMenus} />

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
			New API
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

<!-- Data Table -->
<DataTable
	{columns}
	data={tableData}
	loading={isLoading}
	emptyMessage="No APIs found. Create one to get started."
	rowKey="id"
	onRowClick={openDrawer}
>
	{#snippet cell({ row, column })}
		{#if column.key === 'name'}
			<span class="font-medium text-blue-600 hover:text-blue-800">{row.name}</span>
		{:else if column.key === 'team'}
			<Badge variant="indigo">{row.team}</Badge>
		{:else if column.key === 'routes'}
			<span class="text-gray-600">{row.routes} route{row.routes !== 1 ? 's' : ''}</span>
		{:else if column.key === 'features'}
			<FeatureBadges
				hasRetry={row.hasRetry as boolean}
				hasCircuitBreaker={row.hasCircuitBreaker as boolean}
				hasOutlierDetection={row.hasOutlierDetection as boolean}
				hasHealthCheck={row.hasHealthCheck as boolean}
			/>
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

			<!-- Virtual Hosts -->
			{#if selectedRoute.config?.virtualHosts}
				<ConfigCard title="Virtual Hosts" variant="gray" collapsible defaultCollapsed>
					<div class="space-y-4">
						{#each selectedRoute.config.virtualHosts as vh}
							<div class="p-3 bg-white rounded border border-gray-200">
								<div class="font-medium text-gray-900">{vh.name}</div>
								<div class="text-sm text-gray-500 mt-1">
									Domains: {vh.domains?.join(', ') || '*'}
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
