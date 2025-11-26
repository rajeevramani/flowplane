<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import ResourceTabs from '$lib/components/ResourceTabs.svelte';
	import ExpandableApiRow from '$lib/components/ExpandableApiRow.svelte';
	import ExpandableListenerRow from '$lib/components/ExpandableListenerRow.svelte';
	import ExpandableClusterRow from '$lib/components/ExpandableClusterRow.svelte';
	import ExpandableImportRow from '$lib/components/ExpandableImportRow.svelte';
	import type {
		ImportSummary,
		ListenerResponse,
		RouteResponse,
		ClusterResponse,
		SessionInfoResponse
	} from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import type { Unsubscriber } from 'svelte/store';

	let isLoading = $state(false);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let sessionInfo = $state<SessionInfoResponse | null>(null);
	let currentTeam = $state<string>('');
	let unsubscribe: Unsubscriber;

	// Active tab from URL or default
	let activeTab = $state<string>('apis');

	// Data for each resource type
	let imports = $state<ImportSummary[]>([]);
	let listeners = $state<ListenerResponse[]>([]);
	let routes = $state<RouteResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);

	// Read initial tab from URL
	$effect(() => {
		const urlTab = $page.url.searchParams.get('tab');
		if (urlTab && ['apis', 'listeners', 'clusters', 'imports'].includes(urlTab)) {
			activeTab = urlTab;
		}
	});

	onMount(async () => {
		try {
			sessionInfo = await apiClient.getSessionInfo();

			unsubscribe = selectedTeam.subscribe(async (team) => {
				if (team && team !== currentTeam) {
					currentTeam = team;
					await loadResources(team);
				}
			});
		} catch (err: any) {
			error = err.message || 'Failed to load session info';
		}
	});

	onDestroy(() => {
		if (unsubscribe) {
			unsubscribe();
		}
	});

	async function loadResources(team: string) {
		isLoading = true;
		error = null;

		try {
			const isAdmin = sessionInfo?.isAdmin ?? false;

			const [importsData, listenersData, routesData, clustersData] = await Promise.all([
				isAdmin
					? apiClient.listAllImports()
					: team
						? apiClient.listImports(team)
						: Promise.resolve([]),
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

	function handleTabChange(tabId: string) {
		activeTab = tabId;
		goto(`/resources?tab=${tabId}`, { replaceState: true });
	}

	// Filtered data based on search query
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

	// Tab configuration
	const tabs = $derived([
		{ id: 'apis', label: 'APIs', count: getFilteredRoutes().length },
		{ id: 'listeners', label: 'Listeners', count: getFilteredListeners().length },
		{ id: 'clusters', label: 'Clusters', count: getFilteredClusters().length },
		{ id: 'imports', label: 'Imports', count: getFilteredImports().length }
	]);

	// Delete handlers
	async function handleDeleteRoute(route: RouteResponse) {
		try {
			await apiClient.deleteRoute(route.name);
			await loadResources(currentTeam);
		} catch (err: any) {
			error = err.message || 'Failed to delete route';
		}
	}

	async function handleDeleteListener(listener: ListenerResponse) {
		try {
			await apiClient.deleteListener(listener.name);
			await loadResources(currentTeam);
		} catch (err: any) {
			error = err.message || 'Failed to delete listener';
		}
	}

	async function handleDeleteCluster(cluster: ClusterResponse) {
		try {
			await apiClient.deleteCluster(cluster.name);
			await loadResources(currentTeam);
		} catch (err: any) {
			error = err.message || 'Failed to delete cluster';
		}
	}

	async function handleDeleteImport(importRecord: ImportSummary) {
		try {
			await apiClient.deleteImport(importRecord.id);
			await loadResources(currentTeam);
		} catch (err: any) {
			error = err.message || 'Failed to delete import';
		}
	}
</script>

<!-- Page Header -->
<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="text-2xl font-bold text-gray-900">Resources</h1>
		<p class="mt-1 text-sm text-gray-600">
			View and manage all your API gateway resources
		</p>
	</div>
	<div class="flex gap-3">
		<a
			href="/apis/manage"
			class="inline-flex items-center gap-2 px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-md hover:bg-blue-700 transition-colors"
		>
			<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
			</svg>
			New API
		</a>
		<a
			href="/imports/import"
			class="inline-flex items-center gap-2 px-4 py-2 bg-white text-gray-700 text-sm font-medium rounded-md border border-gray-300 hover:bg-gray-50 transition-colors"
		>
			<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12" />
			</svg>
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
		placeholder="Search resources..."
		class="w-full px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
	/>
</div>

<!-- Tabs -->
<div class="mb-6">
	<ResourceTabs {tabs} {activeTab} onTabChange={handleTabChange} />
</div>

<!-- Content -->
<div class="bg-white rounded-lg shadow-sm border border-gray-200">
	{#if isLoading}
		<div class="flex justify-center items-center py-12">
			<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
		</div>
	{:else if activeTab === 'apis'}
		<!-- APIs -->
		{#if getFilteredRoutes().length === 0}
			<div class="text-center py-12">
				<svg class="mx-auto h-12 w-12 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10" />
				</svg>
				<p class="mt-4 text-gray-500">No APIs found. Create one to get started.</p>
			</div>
		{:else}
			<div class="flex items-center gap-6 py-2 px-4 bg-gray-50 border-b border-gray-200 text-xs font-medium text-gray-500 uppercase tracking-wide">
				<div class="w-4"></div>
				<div class="w-48">Name</div>
				<div class="w-24">Team</div>
				<div class="w-32">Domains</div>
				<div class="w-28">Routes</div>
				<div class="flex-1">Source</div>
				<div class="w-8"></div>
			</div>
			{#each getFilteredRoutes() as route (route.name)}
				<ExpandableApiRow
					{route}
					{imports}
					{clusters}
					{listeners}
					onDelete={handleDeleteRoute}
				/>
			{/each}
		{/if}

	{:else if activeTab === 'listeners'}
		<!-- Listeners -->
		{#if getFilteredListeners().length === 0}
			<div class="text-center py-12">
				<svg class="mx-auto h-12 w-12 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
				</svg>
				<p class="mt-4 text-gray-500">No listeners found.</p>
			</div>
		{:else}
			<div class="flex items-center gap-6 py-2 px-4 bg-gray-50 border-b border-gray-200 text-xs font-medium text-gray-500 uppercase tracking-wide">
				<div class="w-4"></div>
				<div class="w-48">Name</div>
				<div class="w-24">Team</div>
				<div class="w-40">Address</div>
				<div class="w-24">Protocol</div>
				<div class="w-28">Chains</div>
				<div class="flex-1">Source</div>
				<div class="w-8"></div>
			</div>
			{#each getFilteredListeners() as listener (listener.name)}
				<ExpandableListenerRow
					{listener}
					{routes}
					{imports}
					onDelete={handleDeleteListener}
				/>
			{/each}
		{/if}

	{:else if activeTab === 'clusters'}
		<!-- Clusters -->
		{#if getFilteredClusters().length === 0}
			<div class="text-center py-12">
				<svg class="mx-auto h-12 w-12 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4m0 5c0 2.21-3.582 4-8 4s-8-1.79-8-4" />
				</svg>
				<p class="mt-4 text-gray-500">No clusters found.</p>
			</div>
		{:else}
			<div class="flex items-center gap-6 py-2 px-4 bg-gray-50 border-b border-gray-200 text-xs font-medium text-gray-500 uppercase tracking-wide">
				<div class="w-4"></div>
				<div class="w-48">Service</div>
				<div class="w-24">Team</div>
				<div class="w-40">Endpoint</div>
				<div class="w-28">Endpoints</div>
				<div class="w-28">LB Policy</div>
				<div class="flex-1">Source</div>
				<div class="w-8"></div>
			</div>
			{#each getFilteredClusters() as cluster (cluster.name)}
				<ExpandableClusterRow
					{cluster}
					{routes}
					{imports}
					onDelete={handleDeleteCluster}
				/>
			{/each}
		{/if}

	{:else if activeTab === 'imports'}
		<!-- Imports -->
		{#if getFilteredImports().length === 0}
			<div class="text-center py-12">
				<svg class="mx-auto h-12 w-12 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12" />
				</svg>
				<p class="mt-4 text-gray-500">No imports found. Import an OpenAPI spec to get started.</p>
			</div>
		{:else}
			<div class="flex items-center gap-6 py-2 px-4 bg-gray-50 border-b border-gray-200 text-xs font-medium text-gray-500 uppercase tracking-wide">
				<div class="w-4"></div>
				<div class="w-48">Spec Name</div>
				<div class="w-20">Version</div>
				<div class="w-24">Team</div>
				<div class="w-32">Resources</div>
				<div class="w-28">Imported</div>
				<div class="flex-1">ID</div>
				<div class="w-8"></div>
			</div>
			{#each getFilteredImports() as importRecord (importRecord.id)}
				<ExpandableImportRow
					{importRecord}
					{routes}
					{clusters}
					{listeners}
					onDelete={handleDeleteImport}
				/>
			{/each}
		{/if}
	{/if}
</div>
