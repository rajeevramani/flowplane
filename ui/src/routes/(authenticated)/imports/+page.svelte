<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Plus, Eye, Trash2, FileText, Database, Server, Route as RouteIcon } from 'lucide-svelte';
	import type { ImportSummary, RouteResponse, ClusterResponse, ListenerResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');

	// Data
	let imports = $state<ImportSummary[]>([]);
	let routes = $state<RouteResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);
	let listeners = $state<ListenerResponse[]>([]);

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		if (currentTeam && currentTeam !== value) {
			currentTeam = value;
			loadData();
		} else {
			currentTeam = value;
		}
	});

	onMount(async () => {
		await loadData();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		try {
			const [importsData, routesData, clustersData, listenersData] = await Promise.all([
				currentTeam ? apiClient.listImports(currentTeam) : Promise.resolve([]),
				apiClient.listRouteConfigs(),
				apiClient.listClusters(),
				apiClient.listListeners()
			]);

			imports = importsData;
			routes = routesData;
			clusters = clustersData;
			listeners = listenersData;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Calculate stats
	let stats = $derived({
		totalImports: imports.length,
		totalRoutes: routes.filter(r => imports.some(i => i.id === r.importId)).length,
		totalClusters: clusters.filter(c => imports.some(i => i.id === c.importId)).length,
		totalListeners: new Set(imports.map(i => i.listenerName).filter(Boolean)).size
	});

	// Filter imports
	let filteredImports = $derived(
		searchQuery
			? imports.filter(imp =>
				imp.specName.toLowerCase().includes(searchQuery.toLowerCase()) ||
				imp.team.toLowerCase().includes(searchQuery.toLowerCase()) ||
				imp.id.toLowerCase().includes(searchQuery.toLowerCase())
			)
			: imports
	);

	// Get resource counts for an import
	function getResourceCounts(imp: ImportSummary) {
		return {
			routes: routes.filter(r => r.importId === imp.id).length,
			clusters: clusters.filter(c => c.importId === imp.id).length
		};
	}

	// Format date
	function formatDate(dateStr: string): string {
		if (!dateStr) return 'N/A';
		const d = new Date(dateStr);
		const now = new Date();
		const diffMs = now.getTime() - d.getTime();
		const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

		if (diffDays === 0) return 'Today';
		if (diffDays === 1) return 'Yesterday';
		if (diffDays < 7) return `${diffDays} days ago`;
		if (diffDays < 30) return `${Math.floor(diffDays / 7)} weeks ago`;
		return d.toLocaleDateString();
	}

	// Navigate to create page
	function handleCreate() {
		goto('/imports/import');
	}

	// View details
	function handleView(imp: ImportSummary) {
		goto(`/imports/${encodeURIComponent(imp.id)}`);
	}

	// Delete import
	async function handleDelete(imp: ImportSummary) {
		if (!confirm(`Are you sure you want to delete the import "${imp.specName}"? This will also delete all associated routes, clusters, and listeners.`)) {
			return;
		}

		try {
			await apiClient.deleteImport(imp.id);
			await loadData();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete import';
		}
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">OpenAPI Imports</h1>
		<p class="mt-2 text-sm text-gray-600">
			Manage OpenAPI specifications imported for the <span class="font-medium">{currentTeam}</span> team
		</p>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6">
		<Button onclick={handleCreate} variant="primary">
			<Plus class="h-4 w-4 mr-2" />
			Import OpenAPI
		</Button>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-4 gap-4 mb-6">
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Imports</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalImports}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<FileText class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Routes Created</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalRoutes}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<RouteIcon class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Clusters Created</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalClusters}</p>
				</div>
				<div class="p-3 bg-purple-100 rounded-lg">
					<Database class="h-6 w-6 text-purple-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Listeners Used</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalListeners}</p>
				</div>
				<div class="p-3 bg-orange-100 rounded-lg">
					<Server class="h-6 w-6 text-orange-600" />
				</div>
			</div>
		</div>
	</div>

	<!-- Search -->
	<div class="mb-6">
		<input
			type="text"
			bind:value={searchQuery}
			placeholder="Search by spec name, team, or ID..."
			class="w-full md:w-96 px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		/>
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading imports...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filteredImports.length === 0}
		<!-- Empty State -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
			<FileText class="h-12 w-12 text-gray-400 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">
				{searchQuery ? 'No imports found' : 'No OpenAPI imports yet'}
			</h3>
			<p class="text-sm text-gray-600 mb-6">
				{searchQuery
					? 'Try adjusting your search query'
					: 'Get started by importing your first OpenAPI specification'}
			</p>
			{#if !searchQuery}
				<Button onclick={handleCreate} variant="primary">
					<Plus class="h-4 w-4 mr-2" />
					Import OpenAPI
				</Button>
			{/if}
		</div>
	{:else}
		<!-- Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Spec Name
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Version
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Team
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Resources
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Listener
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Imported
						</th>
						<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
							Actions
						</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each filteredImports as imp}
						{@const resourceCounts = getResourceCounts(imp)}
						<tr class="hover:bg-gray-50 transition-colors">
							<!-- Spec Name -->
							<td class="px-6 py-4">
								<div class="flex flex-col">
									<span class="text-sm font-medium text-gray-900">{imp.specName}</span>
									<span class="text-xs text-gray-500 font-mono mt-0.5">{imp.id}</span>
								</div>
							</td>

							<!-- Version -->
							<td class="px-6 py-4">
								{#if imp.specVersion}
									<Badge variant="gray">v{imp.specVersion}</Badge>
								{:else}
									<span class="text-sm text-gray-400">-</span>
								{/if}
							</td>

							<!-- Team -->
							<td class="px-6 py-4">
								<Badge variant="indigo">{imp.team}</Badge>
							</td>

							<!-- Resources -->
							<td class="px-6 py-4">
								<div class="flex flex-col gap-1">
									<span class="text-sm text-gray-600">{resourceCounts.routes} routes</span>
									<span class="text-sm text-gray-600">{resourceCounts.clusters} clusters</span>
								</div>
							</td>

							<!-- Listener -->
							<td class="px-6 py-4">
								{#if imp.listenerName}
									<span class="text-sm font-mono text-gray-900">{imp.listenerName}</span>
								{:else}
									<span class="text-sm text-gray-400">None</span>
								{/if}
							</td>

							<!-- Imported -->
							<td class="px-6 py-4">
								<span class="text-sm text-gray-500">{formatDate(imp.importedAt)}</span>
							</td>

							<!-- Actions -->
							<td class="px-6 py-4 text-right">
								<div class="flex justify-end gap-2">
									<button
										onclick={() => handleView(imp)}
										class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
										title="View details"
									>
										<Eye class="h-4 w-4" />
									</button>
									<button
										onclick={() => handleDelete(imp)}
										class="p-2 text-red-600 hover:bg-red-50 rounded-md transition-colors"
										title="Delete import"
									>
										<Trash2 class="h-4 w-4" />
									</button>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		<!-- Pagination (placeholder for future) -->
		{#if filteredImports.length > 50}
			<div class="mt-4 flex justify-center">
				<p class="text-sm text-gray-600">Showing {filteredImports.length} imports</p>
			</div>
		{/if}
	{/if}
</div>
