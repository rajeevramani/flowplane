<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { listRouteViews } from '$lib/api/route-views';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import {
		Plus,
		FileUp,
		Edit,
		Trash2,
		Server,
		Globe,
		Filter,
		Bot,
		ChevronDown,
		ChevronRight,
		Search,
		LayoutGrid,
		List
	} from 'lucide-svelte';
	import type {
		RouteResponse,
		ClusterResponse,
		ListenerResponse,
		ImportSummary,
		McpTool,
		VirtualHostSummary,
		RouteSummary,
		McpStatus,
		EnableMcpRequest
	} from '$lib/api/types';
	import type {
		RouteListViewDto,
		RouteListStatsDto,
		RouteListQueryParams
	} from '$lib/types/route-view';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import Pagination from '$lib/components/Pagination.svelte';
	import { McpBadge, McpEnableModal, McpQuickToggle } from '$lib/components/mcp';

	// View mode: 'grouped' (hierarchical) or 'flat' (table)
	let viewMode = $state<'grouped' | 'flat'>('flat');

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');

	// Filters for flat view
	let mcpFilter = $state<'all' | 'enabled' | 'disabled'>('all');
	let methodFilter = $state<string>('all');

	// Pagination for flat view
	let currentPage = $state(1);
	let pageSize = $state(20);
	let totalItems = $state(0);
	let totalPages = $state(1);

	// Flat view data
	let flatRoutes = $state<RouteListViewDto[]>([]);
	let flatStats = $state<RouteListStatsDto | null>(null);

	// Grouped view data (existing)
	let routeConfigs = $state<RouteResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);
	let listeners = $state<ListenerResponse[]>([]);
	let imports = $state<ImportSummary[]>([]);

	// Filter counts per route (loaded lazily)
	let routeFilterCounts = $state<Map<string, number>>(new Map());
	let loadingFilters = $state(false);

	// MCP tools data
	let mcpTools = $state<McpTool[]>([]);
	let loadingMcpTools = $state(false);

	// Expanded route configs (for showing virtual hosts and routes)
	let expandedConfigs = $state<Set<string>>(new Set());
	let virtualHostsMap = $state<Map<string, VirtualHostSummary[]>>(new Map());
	let routesMap = $state<Map<string, RouteSummary[]>>(new Map()); // Key: configName_vhName

	// MCP Enable Modal state
	let showMcpModal = $state(false);
	let selectedRoute = $state<{ id: string; path: string; method: string } | null>(null);
	let mcpStatus = $state<McpStatus | null>(null);
	let mcpModalLoading = $state(false);

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		if (currentTeam && currentTeam !== value) {
			currentTeam = value;
			currentPage = 1;
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
			if (viewMode === 'flat') {
				await loadFlatView();
			} else {
				await loadGroupedView();
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	async function loadFlatView() {
		const query: RouteListQueryParams = {
			page: currentPage,
			pageSize: pageSize,
			search: searchQuery || undefined,
			mcpFilter: mcpFilter !== 'all' ? mcpFilter : undefined
		};

		const response = await listRouteViews(query);
		flatRoutes = response.items;
		flatStats = response.stats;
		totalItems = response.pagination.totalCount;
		totalPages = response.pagination.totalPages;
	}

	async function loadGroupedView() {
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

		// Load MCP tools (non-blocking)
		loadMcpTools();
	}

	// Handle view mode change
	function handleViewModeChange(mode: 'grouped' | 'flat') {
		viewMode = mode;
		currentPage = 1;
		loadData();
	}

	// Handle search with debounce
	let searchTimeout: ReturnType<typeof setTimeout>;
	function handleSearchInput() {
		clearTimeout(searchTimeout);
		searchTimeout = setTimeout(() => {
			currentPage = 1;
			loadData();
		}, 300);
	}

	// Handle filter changes
	function handleFilterChange() {
		currentPage = 1;
		loadData();
	}

	// Handle page change
	function handlePageChange(page: number) {
		currentPage = page;
		loadData();
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
					batch.map((route) => apiClient.listRouteConfigFilters(route.name))
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

	// Load MCP tools for the team
	async function loadMcpTools() {
		if (!currentTeam) return;

		loadingMcpTools = true;
		try {
			const response = await apiClient.listMcpTools(currentTeam);
			mcpTools = response.tools;
		} catch (e) {
			console.error('Failed to load MCP tools:', e);
			mcpTools = [];
		} finally {
			loadingMcpTools = false;
		}
	}

	// Get MCP tools associated with routes in a given route config
	function getMcpToolsForConfig(config: RouteResponse): McpTool[] {
		// MCP tools have httpPath, match against routes in this config
		const routePaths = new Set<string>();
		const routeConfig = config.config as Record<string, unknown>;
		const virtualHosts = (routeConfig?.virtualHosts as Array<Record<string, unknown>>) || [];

		for (const vh of virtualHosts) {
			for (const route of (vh.routes as Array<Record<string, unknown>>) || []) {
				const match = route.match as Record<string, unknown>;
				const pathObj = match?.path as Record<string, unknown>;
				const path = (pathObj?.template as string) || (pathObj?.value as string);
				if (path) {
					routePaths.add(path);
				}
			}
		}

		return mcpTools.filter((tool) => {
			if (!tool.httpPath) return false;
			return routePaths.has(tool.httpPath);
		});
	}

	// MCP Stats for grouped view
	let mcpStats = $derived({
		totalTools: mcpTools.length,
		enabledTools: mcpTools.filter((t) => t.enabled).length,
		gatewayApiTools: mcpTools.filter((t) => t.category === 'gateway_api').length,
		learnedSchemas: mcpTools.filter((t) => t.schemaSource === 'learned').length
	});

	// Calculate stats for grouped view
	let groupedStats = $derived({
		totalConfigs: routeConfigs.length,
		totalRoutes: routeConfigs.reduce(
			(sum: number, config: RouteResponse) =>
				sum +
				((config.config as Record<string, unknown>)?.virtualHosts as Array<Record<string, unknown>>)?.reduce(
					(vhSum: number, vh: Record<string, unknown>) => vhSum + ((vh.routes as unknown[]) || []).length,
					0
				) || 0,
			0
		),
		totalDomains: routeConfigs.reduce(
			(sum: number, config: RouteResponse) =>
				sum +
				((config.config as Record<string, unknown>)?.virtualHosts as Array<Record<string, unknown>>)?.reduce(
					(vhSum: number, vh: Record<string, unknown>) => vhSum + ((vh.domains as string[]) || []).length,
					0
				) || 0,
			0
		),
		activeListeners: listeners.length
	});

	// Filter configurations for grouped view
	let filteredConfigs = $derived(
		routeConfigs
			.filter((config: RouteResponse) => config.team === currentTeam)
			.filter(
				(config: RouteResponse) =>
					!searchQuery ||
					config.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
					((config.config as Record<string, unknown>)?.virtualHosts as Array<Record<string, unknown>>)?.some(
						(vh: Record<string, unknown>) =>
							(vh.domains as string[])?.some((domain: string) =>
								domain.toLowerCase().includes(searchQuery.toLowerCase())
							)
					)
			)
	);

	// Filter routes for flat view (team filtering - same pattern as clusters/listeners)
	let filteredFlatRoutes = $derived(
		flatRoutes.filter((route: RouteListViewDto) => route.team === currentTeam)
	);

	// Get route statistics for a configuration
	function getRouteStats(config: RouteResponse) {
		const configObj = config.config as Record<string, unknown>;
		const virtualHosts = (configObj?.virtualHosts as Array<Record<string, unknown>>) || [];
		const allRoutes = virtualHosts.flatMap((vh) => (vh.routes as Array<Record<string, unknown>>) || []);
		const methodCounts: Record<string, number> = {};

		allRoutes.forEach((route: Record<string, unknown>) => {
			const match = route.match as Record<string, unknown>;
			const headers = match?.headers as Array<Record<string, unknown>>;
			const methodHeader = headers?.find((h) => h.name === ':method');
			const method = (methodHeader?.value as string) || 'ANY';
			methodCounts[method] = (methodCounts[method] || 0) + 1;
		});

		return { total: allRoutes.length, methodCounts };
	}

	// Get domain list for display
	function getDomainList(config: RouteResponse): string[] {
		const configObj = config.config as Record<string, unknown>;
		const virtualHosts = (configObj?.virtualHosts as Array<Record<string, unknown>>) || [];
		return virtualHosts.flatMap((vh) => (vh.domains as string[]) || []);
	}

	// Get source type (Native, Gateway, or OpenAPI Import)
	function getSourceType(config: RouteResponse): { type: string; name: string } {
		if (config.importId) {
			const importRecord = imports.find((i) => i.id === config.importId);
			return {
				type: 'import',
				name: importRecord?.specName || 'OpenAPI Import'
			};
		}
		return { type: 'manual', name: 'Manual' };
	}

	// Format date
	function formatDate(date: string | null | undefined): string {
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
	async function handleDelete(config: RouteResponse) {
		if (
			!confirm(`Are you sure you want to delete "${config.name}"? This action cannot be undone.`)
		) {
			return;
		}

		try {
			await apiClient.deleteRouteConfig(config.name);
			await loadData();
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to delete configuration';
		}
	}

	// Delete configuration by name (for flat view)
	async function handleDeleteByName(configName: string) {
		if (
			!confirm(
				`Are you sure you want to delete "${configName}"? This action cannot be undone.`
			)
		) {
			return;
		}

		try {
			await apiClient.deleteRouteConfig(configName);
			await loadData();
		} catch (err: unknown) {
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

	// Toggle route config expansion
	async function toggleConfigExpansion(configName: string) {
		const isExpanded = expandedConfigs.has(configName);
		if (isExpanded) {
			expandedConfigs.delete(configName);
			expandedConfigs = new Set(expandedConfigs);
		} else {
			expandedConfigs.add(configName);
			expandedConfigs = new Set(expandedConfigs);

			// Load virtual hosts if not already loaded
			if (!virtualHostsMap.has(configName)) {
				try {
					const vhosts = await apiClient.listVirtualHosts(configName);
					virtualHostsMap.set(configName, vhosts);
					virtualHostsMap = new Map(virtualHostsMap);
				} catch (err) {
					console.error('Failed to load virtual hosts:', err);
				}
			}
		}
	}

	// Toggle virtual host expansion
	async function toggleVirtualHostExpansion(configName: string, vhName: string) {
		const key = `${configName}_${vhName}`;
		const isExpanded = routesMap.has(key);

		if (isExpanded) {
			routesMap.delete(key);
			routesMap = new Map(routesMap);
		} else {
			try {
				const routes = await apiClient.listRoutesInVirtualHost(configName, vhName);
				routesMap.set(key, routes);
				routesMap = new Map(routesMap);
			} catch (err) {
				console.error('Failed to load routes:', err);
			}
		}
	}

	// Open MCP Enable Modal
	async function handleEnableMcp(routeId: string, path: string, method: string = 'ANY') {
		selectedRoute = { id: routeId, path, method };
		mcpModalLoading = true;
		showMcpModal = true;

		try {
			mcpStatus = await apiClient.getMcpStatus(currentTeam, routeId);
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load MCP status';
			showMcpModal = false;
		} finally {
			mcpModalLoading = false;
		}
	}

	// Handle MCP enable request
	async function handleMcpEnable(request: EnableMcpRequest) {
		if (!selectedRoute) return;

		mcpModalLoading = true;
		try {
			await apiClient.enableMcp(currentTeam, selectedRoute.id, request);
			showMcpModal = false;
			selectedRoute = null;
			mcpStatus = null;

			// Reload data
			await loadData();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to enable MCP';
		} finally {
			mcpModalLoading = false;
		}
	}

	// Close MCP modal
	function handleMcpModalClose() {
		showMcpModal = false;
		selectedRoute = null;
		mcpStatus = null;
	}

	// Handle MCP toggle from flat view
	function handleMcpToggle() {
		loadData();
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8 flex items-start justify-between">
		<div>
			<h1 class="text-3xl font-bold text-gray-900">Routes</h1>
			<p class="mt-2 text-sm text-gray-600">
				Manage API routes for the <span class="font-medium">{currentTeam}</span> team
			</p>
		</div>

		<!-- View Toggle -->
		<div class="flex border border-gray-300 rounded-md overflow-hidden">
			<button
				onclick={() => handleViewModeChange('flat')}
				class="px-3 py-2 text-sm flex items-center gap-1.5 transition-colors
					{viewMode === 'flat'
					? 'bg-blue-600 text-white'
					: 'bg-white text-gray-700 hover:bg-gray-50'}"
				title="Flat table view"
			>
				<List class="h-4 w-4" />
				Flat
			</button>
			<button
				onclick={() => handleViewModeChange('grouped')}
				class="px-3 py-2 text-sm flex items-center gap-1.5 transition-colors
					{viewMode === 'grouped'
					? 'bg-blue-600 text-white'
					: 'bg-white text-gray-700 hover:bg-gray-50'}"
				title="Grouped by configuration"
			>
				<LayoutGrid class="h-4 w-4" />
				Grouped
			</button>
		</div>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6 flex gap-3">
		<Button onclick={handleCreate} variant="primary">
			<Plus class="h-4 w-4 mr-2" />
			Create Route
		</Button>
		<Button onclick={handleImport} variant="secondary">
			<FileUp class="h-4 w-4 mr-2" />
			Import OpenAPI
		</Button>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-6">
		{#if viewMode === 'flat' && flatStats}
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
				<p class="text-sm text-gray-500">Total Routes</p>
				<p class="text-2xl font-bold text-gray-900">{flatStats.totalRoutes}</p>
			</div>
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
				<p class="text-sm text-gray-500">MCP Enabled</p>
				<p class="text-2xl font-bold text-emerald-600">{flatStats.mcpEnabledCount}</p>
			</div>
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
				<p class="text-sm text-gray-500">Unique Clusters</p>
				<p class="text-2xl font-bold text-gray-900">{flatStats.uniqueClusters}</p>
			</div>
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
				<p class="text-sm text-gray-500">Unique Domains</p>
				<p class="text-2xl font-bold text-gray-900">{flatStats.uniqueDomains}</p>
			</div>
		{:else}
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
				<div class="flex items-center justify-between">
					<div>
						<p class="text-sm font-medium text-gray-600">Configurations</p>
						<p class="text-2xl font-bold text-gray-900">{groupedStats.totalConfigs}</p>
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
						<p class="text-2xl font-bold text-gray-900">{groupedStats.totalRoutes}</p>
					</div>
					<div class="p-3 bg-green-100 rounded-lg">
						<Globe class="h-6 w-6 text-green-600" />
					</div>
				</div>
			</div>

			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
				<div class="flex items-center justify-between">
					<div>
						<p class="text-sm font-medium text-gray-600">MCP Enabled</p>
						<p class="text-2xl font-bold text-emerald-600">{mcpStats.enabledTools}</p>
					</div>
					<div class="p-3 bg-emerald-100 rounded-lg">
						<Bot class="h-6 w-6 text-emerald-600" />
					</div>
				</div>
			</div>

			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
				<div class="flex items-center justify-between">
					<div>
						<p class="text-sm font-medium text-gray-600">Domains</p>
						<p class="text-2xl font-bold text-gray-900">{groupedStats.totalDomains}</p>
					</div>
					<div class="p-3 bg-purple-100 rounded-lg">
						<Globe class="h-6 w-6 text-purple-600" />
					</div>
				</div>
			</div>
		{/if}
	</div>

	<!-- Search and Filters -->
	<div class="mb-6 flex flex-col sm:flex-row gap-4">
		<!-- Search Input -->
		<div class="relative flex-1">
			<Search class="absolute left-3 top-1/2 -translate-y-1/2 h-5 w-5 text-gray-400" />
			<input
				type="text"
				bind:value={searchQuery}
				oninput={handleSearchInput}
				placeholder={viewMode === 'flat'
					? 'Search routes by name, path, domain, or cluster...'
					: 'Search by name or domain...'}
				class="w-full pl-10 pr-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
			/>
		</div>

		{#if viewMode === 'flat'}
			<!-- MCP Status Filter -->
			<select
				bind:value={mcpFilter}
				onchange={handleFilterChange}
				class="px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 bg-white"
			>
				<option value="all">All MCP Status</option>
				<option value="enabled">MCP Enabled</option>
				<option value="disabled">MCP Disabled</option>
			</select>
		{/if}
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading routes...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if viewMode === 'flat'}
		<!-- Flat Table View -->
		{#if filteredFlatRoutes.length === 0}
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
				<Server class="h-12 w-12 text-gray-400 mx-auto mb-4" />
				<h3 class="text-lg font-medium text-gray-900 mb-2">
					{searchQuery || mcpFilter !== 'all' ? 'No routes found' : 'No routes yet'}
				</h3>
				<p class="text-sm text-gray-600 mb-6">
					{searchQuery || mcpFilter !== 'all'
						? 'Try adjusting your search or filters'
						: 'Get started by creating a new route or importing an OpenAPI spec'}
				</p>
				{#if !searchQuery && mcpFilter === 'all'}
					<div class="flex justify-center gap-3">
						<Button onclick={handleCreate} variant="primary">
							<Plus class="h-4 w-4 mr-2" />
							Create Route
						</Button>
						<Button onclick={handleImport} variant="secondary">
							<FileUp class="h-4 w-4 mr-2" />
							Import OpenAPI
						</Button>
					</div>
				{/if}
			</div>
		{:else}
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-x-auto">
				<table class="min-w-full divide-y divide-gray-200">
					<thead class="bg-gray-50">
						<tr>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Route
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Path
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Cluster
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Domains
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Method
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								MCP
							</th>
							<th
								class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Actions
							</th>
						</tr>
					</thead>
					<tbody class="bg-white divide-y divide-gray-200">
						{#each filteredFlatRoutes as route}
							<tr class="hover:bg-gray-50 transition-colors">
								<!-- Route Name -->
								<td class="px-6 py-4">
									<div class="text-sm font-medium text-gray-900">{route.routeName}</div>
									<div class="text-xs text-gray-500">
										{route.routeConfigName} / {route.virtualHostName}
									</div>
								</td>

								<!-- Path -->
								<td class="px-6 py-4">
									<code
										class="text-sm bg-gray-100 px-2 py-1 rounded font-mono text-gray-700"
									>
										{route.pathPattern}
									</code>
								</td>

								<!-- Cluster -->
								<td class="px-6 py-4">
									{#if route.upstreamCluster}
										<Badge variant="blue">{route.upstreamCluster}</Badge>
									{:else}
										<span class="text-xs text-gray-400">-</span>
									{/if}
								</td>

								<!-- Domains -->
								<td class="px-6 py-4">
									<div class="flex flex-col gap-1">
										{#if route.domains.length > 0}
											<a
												href="#"
												class="text-sm text-indigo-600 hover:text-indigo-800"
											>
												{route.domains[0]}
											</a>
											{#if route.domains.length > 1}
												<span class="text-xs text-gray-500"
													>+{route.domains.length - 1} more</span
												>
											{/if}
										{:else}
											<span class="text-xs text-gray-400">-</span>
										{/if}
									</div>
								</td>

								<!-- Methods -->
								<td class="px-6 py-4">
									{#if route.httpMethods.length > 0}
										<span class="text-sm text-gray-700">
											{route.httpMethods.join(', ')}
										</span>
									{:else}
										<span class="text-sm text-gray-500">ANY</span>
									{/if}
								</td>

								<!-- MCP Toggle -->
								<td class="px-6 py-4">
									<McpQuickToggle
										{route}
										team={currentTeam}
										onToggle={handleMcpToggle}
										onEnableMcp={(routeId, path) =>
											handleEnableMcp(
												routeId,
												path,
												route.httpMethods[0] || 'ANY'
											)}
									/>
								</td>

								<!-- Actions -->
								<td class="px-6 py-4 text-right">
									<div class="flex justify-end gap-2">
										<button
											onclick={() => goto(`/routes/${encodeURIComponent(route.routeId)}/edit`)}
											class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
											title="Edit route"
										>
											<Edit class="h-4 w-4" />
										</button>
										<button
											onclick={() => handleDeleteByName(route.routeConfigName)}
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

				<!-- Pagination -->
				{#if totalPages > 1}
					<div class="border-t border-gray-200 px-6 py-4">
						<Pagination
							{currentPage}
							{totalPages}
							totalItems={totalItems}
							{pageSize}
							onPageChange={handlePageChange}
						/>
					</div>
				{:else}
					<div class="border-t border-gray-200 px-6 py-4 text-sm text-gray-500">
						Showing {filteredFlatRoutes.length} of {totalItems} routes
					</div>
				{/if}
			</div>
		{/if}
	{:else}
		<!-- Grouped View (existing hierarchical view) -->
		{#if filteredConfigs.length === 0}
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
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-x-auto">
				<table class="min-w-full divide-y divide-gray-200">
					<thead class="bg-gray-50">
						<tr>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Configuration
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Team
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Domains
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Routes
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Filters
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								MCP
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Source
							</th>
							<th
								class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
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
							{@const configMcpTools = getMcpToolsForConfig(config)}
							{@const enabledMcpCount = configMcpTools.filter((t) => t.enabled).length}
							{@const isExpanded = expandedConfigs.has(config.name)}
							{@const virtualHosts = virtualHostsMap.get(config.name) || []}

							<!-- Main route config row -->
							<tr class="hover:bg-gray-50 transition-colors">
								<!-- Configuration Name with expand button -->
								<td class="px-6 py-4">
									<div class="flex items-center gap-2">
										<button
											onclick={() => toggleConfigExpansion(config.name)}
											class="p-1 hover:bg-gray-200 rounded transition-colors"
											title={isExpanded ? 'Collapse' : 'Expand to view routes'}
										>
											{#if isExpanded}
												<ChevronDown class="h-4 w-4 text-gray-600" />
											{:else}
												<ChevronRight class="h-4 w-4 text-gray-600" />
											{/if}
										</button>
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
												<span class="text-xs text-gray-500"
													>+{domains.length - 1} more</span
												>
											{/if}
										{:else}
											<span class="text-sm text-gray-400">No domains</span>
										{/if}
									</div>
								</td>

								<!-- Routes -->
								<td class="px-6 py-4">
									<div class="flex flex-wrap gap-1">
										<span class="text-sm font-medium text-gray-900"
											>{routeStats.total} routes</span
										>
										{#if routeStats.total > 0}
											<div class="flex gap-1 ml-2">
												{#each Object.entries(routeStats.methodCounts) as [method, count]}
													<Badge variant="gray" size="sm">
														{method}
														{count}
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

								<!-- MCP -->
								<td class="px-6 py-4">
									{#if loadingMcpTools}
										<span class="text-xs text-gray-400">Loading...</span>
									{:else if configMcpTools.length > 0}
										<a
											href="/mcp-tools"
											class="inline-flex items-center gap-2"
											title="View MCP tools for this configuration"
										>
											{#if enabledMcpCount > 0}
												<McpBadge status="enabled" />
											{:else}
												<McpBadge status="ready" />
											{/if}
											<span class="text-xs text-gray-500">
												{enabledMcpCount}/{configMcpTools.length}
											</span>
										</a>
									{:else}
										<span class="text-xs text-gray-400">-</span>
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

							<!-- Expanded virtual hosts and routes -->
							{#if isExpanded}
								{#each virtualHosts as vhost}
									{@const vhKey = `${config.name}_${vhost.name}`}
									{@const vhRoutes = routesMap.get(vhKey) || []}
									{@const vhExpanded = routesMap.has(vhKey)}

									<!-- Virtual Host row -->
									<tr class="bg-blue-50">
										<td colspan="8" class="px-6 py-3">
											<div class="flex items-center gap-3">
												<button
													onclick={() =>
														toggleVirtualHostExpansion(config.name, vhost.name)}
													class="p-1 hover:bg-blue-100 rounded transition-colors"
													title={vhExpanded
														? 'Collapse'
														: 'Expand to view individual routes'}
												>
													{#if vhExpanded}
														<ChevronDown class="h-4 w-4 text-blue-600" />
													{:else}
														<ChevronRight class="h-4 w-4 text-blue-600" />
													{/if}
												</button>
												<div class="flex items-center gap-4">
													<span class="text-sm font-medium text-blue-900"
														>Virtual Host: {vhost.name}</span
													>
													<div class="flex gap-2">
														<Badge variant="blue" size="sm"
															>{vhost.routeCount} routes</Badge
														>
														{#each vhost.domains as domain}
															<Badge variant="gray" size="sm">{domain}</Badge>
														{/each}
													</div>
												</div>
											</div>
										</td>
									</tr>

									<!-- Individual routes -->
									{#if vhExpanded}
										{#each vhRoutes as route}
											{@const routeMcpTool = mcpTools.find(
												(t) => t.routeId === route.id
											)}
											<tr class="bg-gray-50">
												<td colspan="8" class="px-6 py-2">
													<div class="flex items-center justify-between pl-12">
														<div class="flex items-center gap-4">
															<Badge variant="gray" size="sm"
																>{route.matchType}</Badge
															>
															<span class="text-sm font-mono text-gray-700"
																>{route.pathPattern}</span
															>
															<span class="text-xs text-gray-500"
																>{route.name}</span
															>
														</div>
														<div class="flex items-center gap-2">
															{#if routeMcpTool}
																{#if routeMcpTool.enabled}
																	<McpBadge status="enabled" />
																{:else}
																	<McpBadge status="ready" />
																{/if}
															{:else}
																<Button
																	variant="secondary"
																	size="sm"
																	onclick={() =>
																		handleEnableMcp(
																			route.id,
																			route.pathPattern,
																			route.matchType
																		)}
																>
																	<Bot class="h-3 w-3 mr-1" />
																	Enable MCP
																</Button>
															{/if}
														</div>
													</div>
												</td>
											</tr>
										{/each}
									{/if}
								{/each}
							{/if}
						{/each}
					</tbody>
				</table>
			</div>

			<!-- Pagination (placeholder for grouped view) -->
			{#if filteredConfigs.length > 50}
				<div class="mt-4 flex justify-center">
					<p class="text-sm text-gray-600">Showing {filteredConfigs.length} configurations</p>
				</div>
			{/if}
		{/if}
	{/if}
</div>

<!-- MCP Enable Modal -->
{#if showMcpModal && selectedRoute && mcpStatus}
	<McpEnableModal
		show={showMcpModal}
		status={mcpStatus}
		routePath={selectedRoute.path}
		routeMethod={selectedRoute.method}
		onClose={handleMcpModalClose}
		onEnable={handleMcpEnable}
		loading={mcpModalLoading}
	/>
{/if}
