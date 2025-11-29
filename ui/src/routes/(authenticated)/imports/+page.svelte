<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import { Plus, Eye, Trash2, MoreVertical } from 'lucide-svelte';
	import DataTable from '$lib/components/DataTable.svelte';
	import StatusIndicator from '$lib/components/StatusIndicator.svelte';
	import DetailDrawer from '$lib/components/DetailDrawer.svelte';
	import ConfigCard from '$lib/components/ConfigCard.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import type {
		ImportSummary,
		ImportDetailsResponse,
		RouteResponse,
		ClusterResponse,
		ListenerResponse,
		SessionInfoResponse
	} from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import type { Unsubscriber } from 'svelte/store';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');
	let sessionInfo = $state<SessionInfoResponse | null>(null);
	let unsubscribe: Unsubscriber;

	// Data
	let imports = $state<ImportSummary[]>([]);
	let routes = $state<RouteResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);
	let listeners = $state<ListenerResponse[]>([]);

	// Drawer state
	let drawerOpen = $state(false);
	let selectedImport = $state<ImportSummary | null>(null);
	let importDetails = $state<ImportDetailsResponse | null>(null);
	let loadingDetails = $state(false);

	// Action menu state
	let openMenuId = $state<string | null>(null);

	onMount(async () => {
		try {
			sessionInfo = await apiClient.getSessionInfo();
		} catch {
			// Ignore
		}

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
			const isAdmin = sessionInfo?.isAdmin ?? false;

			const [importsData, routesData, clustersData, listenersData] = await Promise.all([
				isAdmin ? apiClient.listAllImports() : apiClient.listImports(currentTeam),
				apiClient.listRoutes(),
				apiClient.listClusters(),
				apiClient.listListeners()
			]);

			imports = importsData;
			routes = routesData;
			clusters = clustersData;
			listeners = listenersData;
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Table columns
	const columns = [
		{ key: 'specName', label: 'Spec Name', sortable: true },
		{ key: 'version', label: 'Version' },
		{ key: 'team', label: 'Team', sortable: true },
		{ key: 'resources', label: 'Resources' },
		{ key: 'listener', label: 'Listener' },
		{ key: 'status', label: 'Status' },
		{ key: 'importedAt', label: 'Imported', sortable: true }
	];

	// Transform imports for table display
	let tableData = $derived(
		imports
			.filter((imp) => {
				// Filter by team if a team is selected (non-admin users)
				if (!sessionInfo?.isAdmin && currentTeam && imp.team !== currentTeam) return false;

				// Filter by search query
				if (!searchQuery) return true;
				const query = searchQuery.toLowerCase();
				return (
					imp.specName.toLowerCase().includes(query) ||
					imp.team.toLowerCase().includes(query) ||
					imp.id.toLowerCase().includes(query)
				);
			})
			.map((imp) => {
				// Count resources
				const routeCount = routes.filter((r) => r.importId === imp.id).length;
				const clusterCount = clusters.filter((c) => c.importId === imp.id).length;

				return {
					id: imp.id,
					specName: imp.specName,
					version: imp.specVersion || '-',
					team: imp.team,
					routeCount,
					clusterCount,
					listenerName: imp.listenerName,
					importedAt: imp.importedAt,
					_raw: imp
				};
			})
	);

	async function openDrawer(row: Record<string, unknown>) {
		selectedImport = (row._raw as ImportSummary) || null;
		drawerOpen = true;

		if (selectedImport) {
			loadingDetails = true;
			try {
				importDetails = await apiClient.getImport(selectedImport.id);
			} catch (err) {
				console.error('Failed to load import details:', err);
			} finally {
				loadingDetails = false;
			}
		}
	}

	function closeDrawer() {
		drawerOpen = false;
		selectedImport = null;
		importDetails = null;
	}

	async function handleDelete(importId: string, specName: string) {
		if (!confirm(`Are you sure you want to delete the import "${specName}"? This will also delete all associated routes, clusters, and listeners.`)) {
			return;
		}

		try {
			await apiClient.deleteImport(importId);
			closeDrawer();
			await loadData();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete import';
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

	function formatDate(dateStr: string): string {
		const date = new Date(dateStr);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	// Get resources for an import
	function getImportResources(imp: ImportSummary) {
		return {
			routes: routes.filter((r) => r.importId === imp.id),
			clusters: clusters.filter((c) => c.importId === imp.id),
			listeners: listeners.filter((l) => l.importId === imp.id)
		};
	}
</script>

<svelte:window onclick={closeAllMenus} />

<!-- Page Header -->
<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="text-2xl font-bold text-gray-900">Imports</h1>
		<p class="mt-1 text-sm text-gray-600">OpenAPI specifications imported into the gateway</p>
	</div>
	<a
		href="/imports/import"
		class="inline-flex items-center gap-2 px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-md hover:bg-blue-700 transition-colors"
	>
		<Plus class="h-4 w-4" />
		Import OpenAPI
	</a>
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
		placeholder="Search imports..."
		class="w-full max-w-md px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
	/>
</div>

<!-- Data Table -->
<DataTable
	{columns}
	data={tableData}
	loading={isLoading}
	emptyMessage="No imports found. Import an OpenAPI spec to get started."
	rowKey="id"
	onRowClick={openDrawer}
>
	{#snippet cell({ row, column })}
		{#if column.key === 'specName'}
			<span class="font-medium text-blue-600 hover:text-blue-800">{row.specName}</span>
		{:else if column.key === 'version'}
			{#if row.version !== '-'}
				<Badge variant="gray">v{row.version}</Badge>
			{:else}
				<span class="text-gray-400">-</span>
			{/if}
		{:else if column.key === 'team'}
			<Badge variant="indigo">{row.team}</Badge>
		{:else if column.key === 'resources'}
			<div class="text-sm">
				<span class="text-gray-600">{row.routeCount} routes</span>
				<span class="text-gray-400 mx-1">·</span>
				<span class="text-gray-600">{row.clusterCount} clusters</span>
			</div>
		{:else if column.key === 'listener'}
			{#if row.listenerName}
				<span class="font-mono text-gray-600">{row.listenerName}</span>
			{:else}
				<span class="text-gray-400">-</span>
			{/if}
		{:else if column.key === 'status'}
			<StatusIndicator status="active" label="Imported" />
		{:else if column.key === 'importedAt'}
			<span class="text-gray-600 text-sm">{formatDate(row.importedAt as string)}</span>
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
						onclick={() => handleDelete(row.id as string, row.specName as string)}
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
	title={selectedImport?.specName || ''}
	subtitle={selectedImport
		? `${selectedImport.specVersion ? `v${selectedImport.specVersion} · ` : ''}Team: ${selectedImport.team}`
		: undefined}
	onClose={closeDrawer}
>
	{#if selectedImport}
		<div class="space-y-6">
			{#if loadingDetails}
				<div class="flex justify-center py-8">
					<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				</div>
			{:else}
				<!-- Overview -->
				<ConfigCard title="Overview" variant="gray">
					<dl class="grid grid-cols-2 gap-4 text-sm">
						<div>
							<dt class="text-gray-500">Import ID</dt>
							<dd class="font-mono text-gray-900 text-xs">{selectedImport.id}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Listener</dt>
							<dd class="text-gray-900">{selectedImport.listenerName || 'None'}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Imported</dt>
							<dd class="text-gray-900">{formatDate(selectedImport.importedAt)}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Updated</dt>
							<dd class="text-gray-900">{formatDate(selectedImport.updatedAt)}</dd>
						</div>
					</dl>
				</ConfigCard>

				<!-- Resource counts -->
				{#if importDetails}
					<ConfigCard title="Resources" variant="blue">
						<div class="grid grid-cols-3 gap-4">
							<div class="text-center p-3 bg-white rounded border border-blue-200">
								<div class="text-2xl font-bold text-blue-600">{importDetails.routeCount}</div>
								<div class="text-sm text-gray-600">Routes</div>
							</div>
							<div class="text-center p-3 bg-white rounded border border-blue-200">
								<div class="text-2xl font-bold text-blue-600">{importDetails.clusterCount}</div>
								<div class="text-sm text-gray-600">Clusters</div>
							</div>
							<div class="text-center p-3 bg-white rounded border border-blue-200">
								<div class="text-2xl font-bold text-blue-600">{importDetails.listenerCount}</div>
								<div class="text-sm text-gray-600">Listeners</div>
							</div>
						</div>
					</ConfigCard>
				{/if}

				<!-- Routes List -->
				{@const resources = getImportResources(selectedImport)}
				{#if resources.routes.length > 0}
					<ConfigCard title="Routes" variant="green" collapsible>
						<div class="space-y-2">
							{#each resources.routes as route}
								<div class="p-2 bg-white rounded border border-green-200">
									<div class="font-medium text-gray-900">{route.name}</div>
									<div class="text-sm text-gray-500">{route.pathPrefix}</div>
								</div>
							{/each}
						</div>
					</ConfigCard>
				{/if}

				<!-- Clusters List -->
				{#if resources.clusters.length > 0}
					<ConfigCard title="Clusters" variant="yellow" collapsible defaultCollapsed>
						<div class="space-y-2">
							{#each resources.clusters as cluster}
								<div class="p-2 bg-white rounded border border-yellow-200">
									<div class="font-medium text-gray-900">{cluster.serviceName}</div>
									<div class="text-sm text-gray-500">{cluster.name}</div>
								</div>
							{/each}
						</div>
					</ConfigCard>
				{/if}
			{/if}
		</div>
	{/if}

	{#snippet footer()}
		<div class="flex justify-end gap-3">
			<Button variant="ghost" onclick={closeDrawer}>Close</Button>
			<Button
				variant="danger"
				onclick={() => selectedImport && handleDelete(selectedImport.id, selectedImport.specName)}
			>
				Delete Import
			</Button>
		</div>
	{/snippet}
</DetailDrawer>
