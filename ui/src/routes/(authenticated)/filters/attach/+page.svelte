<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { ArrowLeft } from 'lucide-svelte';
	import type { FilterResponse, RouteResponse, VirtualHostSummary, RouteSummary } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import FilterSearchDropdown from '$lib/components/filters/FilterSearchDropdown.svelte';
	import ResourceSelector, {
		type RouteConfigWithFilters,
		type VirtualHostWithFilters,
		type RouteWithFilters
	} from '$lib/components/filters/ResourceSelector.svelte';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);
	let successMessage = $state<string | null>(null);
	let isApplying = $state(false);
	let currentTeam = $state<string>('');

	// Data
	let filters = $state<FilterResponse[]>([]);
	let routeConfigs = $state<RouteResponse[]>([]);
	let routeConfigsWithFilters = $state<RouteConfigWithFilters[]>([]);
	let virtualHostsWithFilters = $state<VirtualHostWithFilters[]>([]);
	let routesWithFilters = $state<RouteWithFilters[]>([]);

	// Selection state
	let selectedFilterId = $state<string | null>(null);
	let selectedRouteConfigs = $state<Set<string>>(new Set());
	let selectedVirtualHosts = $state<Set<string>>(new Set());
	let selectedRoutes = $state<Set<string>>(new Set());

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
		// Check for filterId query parameter to pre-select a filter
		const queryFilterId = $page.url.searchParams.get('filterId');
		if (queryFilterId) {
			const filter = filters.find(f => f.id === queryFilterId);
			if (filter) {
				await handleFilterSelect(filter);
			}
		}
	});

	async function loadData() {
		isLoading = true;
		error = null;
		successMessage = null;

		try {
			// Load filters and route configs in parallel
			const [filtersData, routeConfigsData] = await Promise.all([
				apiClient.listFilters(),
				apiClient.listRouteConfigs()
			]);

			// Filter by current team
			filters = filtersData.filter((f) => f.team === currentTeam);
			routeConfigs = routeConfigsData.filter((r) => r.team === currentTeam);

			// Build route config data with virtual host and route counts
			routeConfigsWithFilters = await Promise.all(
				routeConfigs.map(async (rc) => {
					// Get virtual hosts for this route config
					let virtualHostCount = 0;
					let routeCount = 0;
					let attachedFilters: FilterResponse[] = [];

					try {
						const vhs = await apiClient.listVirtualHosts(rc.name);
						virtualHostCount = vhs.length;

						// Count total routes across all virtual hosts
						for (const vh of vhs) {
							routeCount += vh.routeCount;
						}

						// Get attached filters at route config level
						const rcFilters = await apiClient.listRouteConfigFilters(rc.name);
						attachedFilters = rcFilters.filters || [];
					} catch {
						// Ignore errors for individual route configs
					}

					return {
						name: rc.name,
						team: rc.team,
						virtualHostCount,
						routeCount,
						isAttached: false // Will be updated when a filter is selected
					};
				})
			);

			// Load virtual hosts and routes
			virtualHostsWithFilters = [];
			routesWithFilters = [];

			for (const rc of routeConfigs) {
				try {
					const vhs = await apiClient.listVirtualHosts(rc.name);
					for (const vh of vhs) {
						virtualHostsWithFilters.push({
							...vh,
							routeConfigName: rc.name,
							isAttached: false
						});

						// Load routes for this virtual host
						try {
							const routes = await apiClient.listRoutesInVirtualHost(rc.name, vh.name);
							for (const route of routes) {
								routesWithFilters.push({
									...route,
									routeConfigName: rc.name,
									virtualHostName: vh.name,
									isAttached: false
								});
							}
						} catch {
							// Ignore errors for individual virtual hosts
						}
					}
				} catch {
					// Ignore errors for individual route configs
				}
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
			console.error('Failed to load data:', e);
		} finally {
			isLoading = false;
		}
	}

	// When a filter is selected, check which resources already have it attached
	async function handleFilterSelect(filter: FilterResponse) {
		selectedFilterId = filter.id;
		selectedRouteConfigs = new Set();
		selectedVirtualHosts = new Set();
		selectedRoutes = new Set();
		successMessage = null;
		actionError = null;

		// Check attachments for route configs
		for (const rc of routeConfigsWithFilters) {
			try {
				const rcFilters = await apiClient.listRouteConfigFilters(rc.name);
				rc.isAttached = (rcFilters.filters || []).some((f) => f.id === filter.id);
			} catch {
				rc.isAttached = false;
			}
		}

		// Force reactivity
		routeConfigsWithFilters = [...routeConfigsWithFilters];

		// Check attachments for virtual hosts
		for (const vh of virtualHostsWithFilters) {
			try {
				const vhFilters = await apiClient.listVirtualHostFilters(vh.routeConfigName, vh.name);
				vh.isAttached = (vhFilters.filters || []).some((f) => f.id === filter.id);
			} catch {
				vh.isAttached = false;
			}
		}
		virtualHostsWithFilters = [...virtualHostsWithFilters];

		// Check attachments for routes
		for (const route of routesWithFilters) {
			try {
				const routeFilters = await apiClient.listRouteHierarchyFilters(
					route.routeConfigName,
					route.virtualHostName,
					route.name
				);
				route.isAttached = (routeFilters.filters || []).some((f) => f.id === filter.id);
			} catch {
				route.isAttached = false;
			}
		}
		routesWithFilters = [...routesWithFilters];
	}

	function handleRouteConfigToggle(name: string) {
		if (selectedRouteConfigs.has(name)) {
			selectedRouteConfigs.delete(name);
		} else {
			selectedRouteConfigs.add(name);
		}
		selectedRouteConfigs = new Set(selectedRouteConfigs);
	}

	function handleVirtualHostToggle(id: string) {
		if (selectedVirtualHosts.has(id)) {
			selectedVirtualHosts.delete(id);
		} else {
			selectedVirtualHosts.add(id);
		}
		selectedVirtualHosts = new Set(selectedVirtualHosts);
	}

	function handleRouteToggle(id: string) {
		if (selectedRoutes.has(id)) {
			selectedRoutes.delete(id);
		} else {
			selectedRoutes.add(id);
		}
		selectedRoutes = new Set(selectedRoutes);
	}

	async function handleDetachRouteConfig(name: string) {
		if (!selectedFilterId) return;

		try {
			await apiClient.detachFilterFromRouteConfig(name, selectedFilterId);
			// Update the UI
			const rc = routeConfigsWithFilters.find((r) => r.name === name);
			if (rc) {
				rc.isAttached = false;
				routeConfigsWithFilters = [...routeConfigsWithFilters];
			}
			successMessage = `Filter detached from route config "${name}"`;
		} catch (e) {
			actionError = e instanceof Error ? e.message : 'Failed to detach filter';
		}
	}

	async function handleDetachVirtualHost(id: string) {
		if (!selectedFilterId) return;

		const vh = virtualHostsWithFilters.find((v) => v.id === id);
		if (!vh) return;

		try {
			await apiClient.detachFilterFromVirtualHost(vh.routeConfigName, vh.name, selectedFilterId);
			vh.isAttached = false;
			virtualHostsWithFilters = [...virtualHostsWithFilters];
			successMessage = `Filter detached from virtual host "${vh.name}"`;
		} catch (e) {
			actionError = e instanceof Error ? e.message : 'Failed to detach filter';
		}
	}

	async function handleDetachRoute(id: string) {
		if (!selectedFilterId) return;

		const route = routesWithFilters.find((r) => r.id === id);
		if (!route) return;

		try {
			await apiClient.detachFilterFromRoute(
				route.routeConfigName,
				route.virtualHostName,
				route.name,
				selectedFilterId
			);
			route.isAttached = false;
			routesWithFilters = [...routesWithFilters];
			successMessage = `Filter detached from route "${route.name}"`;
		} catch (e) {
			actionError = e instanceof Error ? e.message : 'Failed to detach filter';
		}
	}

	async function applyAttachments() {
		if (!selectedFilterId) return;

		isApplying = true;
		actionError = null;
		successMessage = null;

		let attachedCount = 0;
		const errors: string[] = [];

		try {
			// Attach to route configs
			for (const name of selectedRouteConfigs) {
				try {
					await apiClient.attachFilterToRouteConfig(name, { filterId: selectedFilterId });
					attachedCount++;
					const rc = routeConfigsWithFilters.find((r) => r.name === name);
					if (rc) rc.isAttached = true;
				} catch (e) {
					errors.push(`Route config "${name}": ${e instanceof Error ? e.message : 'Failed'}`);
				}
			}

			// Attach to virtual hosts
			for (const id of selectedVirtualHosts) {
				const vh = virtualHostsWithFilters.find((v) => v.id === id);
				if (!vh) continue;

				try {
					await apiClient.attachFilterToVirtualHost(vh.routeConfigName, vh.name, {
						filterId: selectedFilterId
					});
					attachedCount++;
					vh.isAttached = true;
				} catch (e) {
					errors.push(`Virtual host "${vh.name}": ${e instanceof Error ? e.message : 'Failed'}`);
				}
			}

			// Attach to routes
			for (const id of selectedRoutes) {
				const route = routesWithFilters.find((r) => r.id === id);
				if (!route) continue;

				try {
					await apiClient.attachFilterToRoute(
						route.routeConfigName,
						route.virtualHostName,
						route.name,
						{ filterId: selectedFilterId }
					);
					attachedCount++;
					route.isAttached = true;
				} catch (e) {
					errors.push(`Route "${route.name}": ${e instanceof Error ? e.message : 'Failed'}`);
				}
			}

			// Clear selections
			selectedRouteConfigs = new Set();
			selectedVirtualHosts = new Set();
			selectedRoutes = new Set();

			// Trigger reactivity
			routeConfigsWithFilters = [...routeConfigsWithFilters];
			virtualHostsWithFilters = [...virtualHostsWithFilters];
			routesWithFilters = [...routesWithFilters];

			if (errors.length > 0) {
				actionError = `Some attachments failed:\n${errors.join('\n')}`;
			}

			if (attachedCount > 0) {
				successMessage = `Successfully attached filter to ${attachedCount} resource${attachedCount !== 1 ? 's' : ''}`;
			}
		} finally {
			isApplying = false;
		}
	}

	function handleBack() {
		goto('/filters');
	}

	let totalNewSelections = $derived(
		selectedRouteConfigs.size + selectedVirtualHosts.size + selectedRoutes.size
	);
</script>

<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
	<!-- Page Header -->
	<div class="mb-6 flex items-center justify-between">
		<div class="flex items-center gap-4">
			<button
				onclick={handleBack}
				class="p-2 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded-md"
			>
				<ArrowLeft class="w-5 h-5" />
			</button>
			<div>
				<h1 class="text-2xl font-bold text-gray-900">Attach Filters</h1>
				<p class="text-sm text-gray-600 mt-1">
					Select a filter and attach it to resources for the <span class="font-medium">{currentTeam}</span> team
				</p>
			</div>
		</div>
	</div>

	<!-- Error Messages -->
	{#if error}
		<div class="bg-red-50 border border-red-200 rounded-md p-4 mb-6">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{/if}

	{#if actionError}
		<div class="bg-red-50 border border-red-200 rounded-md p-4 mb-6">
			<p class="text-sm text-red-800 whitespace-pre-wrap">{actionError}</p>
		</div>
	{/if}

	{#if successMessage}
		<div class="bg-green-50 border border-green-200 rounded-md p-4 mb-6">
			<p class="text-sm text-green-800">{successMessage}</p>
		</div>
	{/if}

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading filters and resources...</span>
			</div>
		</div>
	{:else}
		<!-- Step 1: Select Filter -->
		<div class="bg-white rounded-lg border border-gray-200 shadow-sm mb-6">
			<div class="px-6 py-4 border-b border-gray-200 bg-gray-50">
				<div class="flex items-center gap-3">
					<span class="flex items-center justify-center w-6 h-6 rounded-full bg-blue-600 text-white text-sm font-medium">1</span>
					<h2 class="text-lg font-semibold text-gray-900">Select Filter</h2>
				</div>
			</div>
			<div class="p-6">
				{#if filters.length === 0}
					<div class="text-center py-8 text-gray-500">
						<p>No filters available for this team.</p>
						<div class="mt-4">
							<Button onclick={() => goto('/filters/create')} variant="primary">
								Create Filter
							</Button>
						</div>
					</div>
				{:else}
					<div class="max-w-md">
						<FilterSearchDropdown
							filters={filters}
							selectedFilterId={selectedFilterId}
							onSelect={handleFilterSelect}
							placeholder="Select a filter to attach..."
						/>
					</div>
				{/if}
			</div>
		</div>

		<!-- Step 2: Select Resources -->
		{#if selectedFilterId}
			<div class="mb-6">
				<div class="px-6 py-4 border-b border-gray-200 bg-gray-50 rounded-t-lg border border-gray-200">
					<div class="flex items-center gap-3">
						<span class="flex items-center justify-center w-6 h-6 rounded-full bg-blue-600 text-white text-sm font-medium">2</span>
						<h2 class="text-lg font-semibold text-gray-900">Select Resources to Attach</h2>
					</div>
				</div>
				<ResourceSelector
					routeConfigs={routeConfigsWithFilters}
					virtualHosts={virtualHostsWithFilters}
					routes={routesWithFilters}
					selectedRouteConfigs={selectedRouteConfigs}
					selectedVirtualHosts={selectedVirtualHosts}
					selectedRoutes={selectedRoutes}
					onRouteConfigToggle={handleRouteConfigToggle}
					onVirtualHostToggle={handleVirtualHostToggle}
					onRouteToggle={handleRouteToggle}
					onDetachRouteConfig={handleDetachRouteConfig}
					onDetachVirtualHost={handleDetachVirtualHost}
					onDetachRoute={handleDetachRoute}
				/>
			</div>

			<!-- Selection Summary -->
			<div class="bg-white rounded-lg border border-gray-200 p-4">
				<div class="flex items-center justify-between">
					<div>
						<h3 class="text-sm font-medium text-gray-900">Selection Summary</h3>
						{#if totalNewSelections > 0}
							<p class="text-sm text-gray-600 mt-1">
								<span class="font-medium text-blue-600">{totalNewSelections} new resource{totalNewSelections !== 1 ? 's' : ''}</span> selected for attachment
							</p>
						{:else}
							<p class="text-sm text-gray-500 mt-1">No resources selected</p>
						{/if}
					</div>
					<div class="flex items-center gap-3">
						<Button onclick={handleBack} variant="secondary">
							Cancel
						</Button>
						<Button
							onclick={applyAttachments}
							variant="primary"
							disabled={totalNewSelections === 0 || isApplying}
						>
							{#if isApplying}
								Applying...
							{:else}
								Apply Attachments
							{/if}
						</Button>
					</div>
				</div>
			</div>
		{/if}
	{/if}
</div>
