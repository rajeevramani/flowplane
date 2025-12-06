<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Plus, FileUp, Edit, Trash2, Server, Globe, Filter } from 'lucide-svelte';
	import type { RouteResponse, ClusterResponse, ListenerResponse, ImportSummary, FilterResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');

	// Data
	let routeConfigs = $state<RouteResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);
	let listeners = $state<ListenerResponse[]>([]);
	let imports = $state<ImportSummary[]>([]);

	// Filter counts per route (loaded lazily)
	let routeFilterCounts = $state<Map<string, number>>(new Map());
	let loadingFilters = $state(false);

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
			const [routesData, clustersData, listenersData, importsData] = await Promise.all([
				apiClient.listRouteConfigs(),
				apiClient.listClusters(),
				apiClient.listListeners(),
				currentTeam ? apiClient.listImports(currentTeam) : Promise.resolve([])
			]);

			routeConfigs = routesData;
			clusters = clustersData;
			listeners = listenersData;
			imports = importsData;

			// Load filter counts for all routes (non-blocking)
			loadFilterCounts(routesData);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Load filter counts for all routes (runs in background)
	async function loadFilterCounts(routes: RouteResponse[]) {
		if (routes.length === 0) return;

		loadingFilters = true;
		const newCounts = new Map<string, number>();

		try {
			// Load filter counts for each route in parallel (with a reasonable limit)
			const batchSize = 10;
			for (let i = 0; i < routes.length; i += batchSize) {
				const batch = routes.slice(i, i + batchSize);
				const results = await Promise.allSettled(
					batch.map(route => apiClient.listRouteConfigFilters(route.name))
				);

				results.forEach((result, index) => {
					const routeName = batch[index].name;
					if (result.status === 'fulfilled') {
						newCounts.set(routeName, result.value.filters.length);
					} else {
						newCounts.set(routeName, 0);
					}
				});

				// Update state after each batch
				routeFilterCounts = new Map(newCounts);
			}
		} catch (e) {
			console.error('Failed to load filter counts:', e);
		} finally {
			loadingFilters = false;
		}
	}

	// Calculate stats
	let stats = $derived({
		totalConfigs: routeConfigs.length,
		totalRoutes: routeConfigs.reduce((sum: number, config: any) =>
			sum + config.config.virtualHosts.reduce((vhSum: number, vh: any) => vhSum + vh.routes.length, 0), 0
		),
		totalDomains: routeConfigs.reduce((sum: number, config: any) =>
			sum + config.config.virtualHosts.reduce((vhSum: number, vh: any) => vhSum + vh.domains.length, 0), 0
		),
		activeListeners: listeners.length
	});

	// Filter configurations
	let filteredConfigs = $derived(
		routeConfigs
			.filter((config: any) => config.team === currentTeam)
			.filter((config: any) =>
				!searchQuery ||
				config.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				config.config.virtualHosts.some((vh: any) =>
					vh.domains.some((domain: string) => domain.toLowerCase().includes(searchQuery.toLowerCase()))
				)
			)
	);

	// Get route statistics for a configuration
	function getRouteStats(config: any) {
		const allRoutes = config.config.virtualHosts.flatMap((vh: any) => vh.routes);
		const methodCounts: Record<string, number> = {};

		allRoutes.forEach((route: any) => {
			const method = route.match.headers?.find((h: any) => h.name === ':method')?.value || 'ANY';
			methodCounts[method] = (methodCounts[method] || 0) + 1;
		});

		return { total: allRoutes.length, methodCounts };
	}

	// Get domain list for display
	function getDomainList(config: any): string[] {
		return config.config.virtualHosts.flatMap((vh: any) => vh.domains);
	}

	// Get source type (Native, Gateway, or OpenAPI Import)
	function getSourceType(config: RouteResponse): { type: string; name: string } {
		if (config.importId) {
			const importRecord = imports.find(i => i.id === config.importId);
			return {
				type: 'import',
				name: importRecord?.specName || 'OpenAPI Import'
			};
		}
		return { type: 'manual', name: 'Manual' };
	}

	// Format date
	function formatDate(date: any): string {
		if (!date) return 'N/A';
		const d = new Date(date);
		const now = new Date();
		const diffMs = now.getTime() - d.getTime();
		const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

		if (diffDays === 0) return 'Today';
		if (diffDays === 1) return 'Yesterday';
		if (diffDays < 7) return `${diffDays} days ago`;
		if (diffDays < 30) return `${Math.floor(diffDays / 7)} weeks ago`;
		return d.toLocaleDateString();
	}

	// Navigate to edit page
	function handleEdit(configName: string) {
		goto(`/route-configs/${encodeURIComponent(configName)}/edit`);
	}

	// Delete configuration
	async function handleDelete(config: any) {
		if (!confirm(`Are you sure you want to delete "${config.name}"? This action cannot be undone.`)) {
			return;
		}

		try {
			await apiClient.deleteRouteConfig(config.name);
			await loadData();
		} catch (err: any) {
			error = err instanceof Error ? err.message : 'Failed to delete configuration';
		}
	}

	// Navigate to create page
	function handleCreate() {
		goto('/route-configs/create');
	}

	// Navigate to import page
	function handleImport() {
		goto('/imports/create');
	}
</script>

<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Route Configurations</h1>
		<p class="mt-2 text-sm text-gray-600">
			Manage API route configurations and virtual hosts for the <span class="font-medium">{currentTeam}</span> team
		</p>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6 flex gap-3">
		<Button onclick={handleCreate} variant="primary">
			<Plus class="h-4 w-4 mr-2" />
			Create Configuration
		</Button>
		<Button onclick={handleImport} variant="secondary">
			<FileUp class="h-4 w-4 mr-2" />
			Import OpenAPI
		</Button>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-4 gap-4 mb-6">
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Configurations</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalConfigs}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<Server class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Routes</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalRoutes}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<Globe class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Domains</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalDomains}</p>
				</div>
				<div class="p-3 bg-purple-100 rounded-lg">
					<Globe class="h-6 w-6 text-purple-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Active Listeners</p>
					<p class="text-2xl font-bold text-gray-900">{stats.activeListeners}</p>
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
			placeholder="Search by name or domain..."
			class="w-full md:w-96 px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		/>
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading configurations...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filteredConfigs.length === 0}
		<!-- Empty State -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
			<Server class="h-12 w-12 text-gray-400 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">
				{searchQuery ? 'No configurations found' : 'No route configurations yet'}
			</h3>
			<p class="text-sm text-gray-600 mb-6">
				{searchQuery
					? 'Try adjusting your search query'
					: 'Get started by creating a new configuration or importing an OpenAPI spec'}
			</p>
			{#if !searchQuery}
				<div class="flex justify-center gap-3">
					<Button onclick={handleCreate} variant="primary">
						<Plus class="h-4 w-4 mr-2" />
						Create Configuration
					</Button>
					<Button onclick={handleImport} variant="secondary">
						<FileUp class="h-4 w-4 mr-2" />
						Import OpenAPI
					</Button>
				</div>
			{/if}
		</div>
	{:else}
		<!-- Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Configuration
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Team
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Domains
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Routes
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Filters
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Source
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Created
						</th>
						<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
							Actions
						</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each filteredConfigs as config}
						{@const routeStats = getRouteStats(config)}
						{@const domains = getDomainList(config)}
						{@const source = getSourceType(config)}
						{@const filterCount = routeFilterCounts.get(config.name)}
						<tr class="hover:bg-gray-50 transition-colors">
							<!-- Configuration Name -->
							<td class="px-6 py-4">
								<div class="flex flex-col">
									<span class="text-sm font-medium text-gray-900">{config.name}</span>
								</div>
							</td>

							<!-- Team -->
							<td class="px-6 py-4">
								<Badge variant="indigo">{config.team}</Badge>
							</td>

							<!-- Domains -->
							<td class="px-6 py-4">
								<div class="flex flex-col gap-1">
									{#if domains.length > 0}
										<span class="text-sm text-gray-900">{domains[0]}</span>
										{#if domains.length > 1}
											<span class="text-xs text-gray-500">+{domains.length - 1} more</span>
										{/if}
									{:else}
										<span class="text-sm text-gray-400">No domains</span>
									{/if}
								</div>
							</td>

							<!-- Routes -->
							<td class="px-6 py-4">
								<div class="flex flex-wrap gap-1">
									<span class="text-sm font-medium text-gray-900">{routeStats.total} routes</span>
									{#if routeStats.total > 0}
										<div class="flex gap-1 ml-2">
											{#each Object.entries(routeStats.methodCounts) as [method, count]}
												<Badge variant="gray" size="sm">
													{method} {count}
												</Badge>
											{/each}
										</div>
									{/if}
								</div>
							</td>

							<!-- Filters -->
							<td class="px-6 py-4">
								{#if filterCount === undefined && loadingFilters}
									<span class="text-xs text-gray-400">Loading...</span>
								{:else if filterCount && filterCount > 0}
									<button
										onclick={() => handleEdit(config.name)}
										class="inline-flex items-center gap-1 px-2 py-1 rounded-full text-xs font-medium bg-blue-100 text-blue-700 hover:bg-blue-200 transition-colors"
										title="View attached filters"
									>
										<Filter class="h-3 w-3" />
										{filterCount}
									</button>
								{:else}
									<span class="text-xs text-gray-400">None</span>
								{/if}
							</td>

							<!-- Source -->
							<td class="px-6 py-4">
								{#if source.type === 'import'}
									<Badge variant="purple">{source.name}</Badge>
								{:else}
									<Badge variant="gray">{source.name}</Badge>
								{/if}
							</td>

							<!-- Created -->
							<td class="px-6 py-4">
								<span class="text-sm text-gray-500">-</span>
							</td>

							<!-- Actions -->
							<td class="px-6 py-4 text-right">
								<div class="flex justify-end gap-2">
									<button
										onclick={() => handleEdit(config.name)}
										class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
										title="Edit configuration"
									>
										<Edit class="h-4 w-4" />
									</button>
									<button
										onclick={() => handleDelete(config)}
										class="p-2 text-red-600 hover:bg-red-50 rounded-md transition-colors"
										title="Delete configuration"
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
		{#if filteredConfigs.length > 50}
			<div class="mt-4 flex justify-center">
				<p class="text-sm text-gray-600">Showing {filteredConfigs.length} configurations</p>
			</div>
		{/if}
	{/if}
</div>
