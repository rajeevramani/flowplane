<script lang="ts">
	import { Layers, Globe, Map, Search } from 'lucide-svelte';
	import type { RouteResponse, VirtualHostSummary, RouteSummary, FilterResponse } from '$lib/api/types';

	// Resource with filter attachment info
	export interface RouteConfigWithFilters {
		name: string;
		team: string;
		virtualHostCount: number;
		routeCount: number;
		isAttached: boolean;
	}

	export interface VirtualHostWithFilters extends VirtualHostSummary {
		routeConfigName: string;
		isAttached: boolean;
	}

	export interface RouteWithFilters extends RouteSummary {
		routeConfigName: string;
		virtualHostName: string;
		isAttached: boolean;
	}

	type TabType = 'route-configs' | 'virtual-hosts' | 'routes';

	interface Props {
		routeConfigs: RouteConfigWithFilters[];
		virtualHosts: VirtualHostWithFilters[];
		routes: RouteWithFilters[];
		selectedRouteConfigs: Set<string>;
		selectedVirtualHosts: Set<string>;
		selectedRoutes: Set<string>;
		onRouteConfigToggle: (name: string) => void;
		onVirtualHostToggle: (id: string) => void;
		onRouteToggle: (id: string) => void;
		onDetachRouteConfig: (name: string) => void;
		onDetachVirtualHost: (id: string) => void;
		onDetachRoute: (id: string) => void;
		showAttached?: boolean;
	}

	let {
		routeConfigs,
		virtualHosts,
		routes,
		selectedRouteConfigs,
		selectedVirtualHosts,
		selectedRoutes,
		onRouteConfigToggle,
		onVirtualHostToggle,
		onRouteToggle,
		onDetachRouteConfig,
		onDetachVirtualHost,
		onDetachRoute,
		showAttached = true
	}: Props = $props();

	let activeTab = $state<TabType>('route-configs');
	let searchQuery = $state('');

	// Filter resources based on search and showAttached toggle
	let filteredRouteConfigs = $derived(
		routeConfigs.filter((rc) => {
			if (!showAttached && rc.isAttached) return false;
			if (!searchQuery) return true;
			return rc.name.toLowerCase().includes(searchQuery.toLowerCase());
		})
	);

	let filteredVirtualHosts = $derived(
		virtualHosts.filter((vh) => {
			if (!showAttached && vh.isAttached) return false;
			if (!searchQuery) return true;
			const query = searchQuery.toLowerCase();
			return (
				vh.name.toLowerCase().includes(query) ||
				vh.routeConfigName.toLowerCase().includes(query) ||
				vh.domains.some((d) => d.toLowerCase().includes(query))
			);
		})
	);

	let filteredRoutes = $derived(
		routes.filter((r) => {
			if (!showAttached && r.isAttached) return false;
			if (!searchQuery) return true;
			const query = searchQuery.toLowerCase();
			return (
				r.name.toLowerCase().includes(query) ||
				r.pathPattern.toLowerCase().includes(query) ||
				r.routeConfigName.toLowerCase().includes(query) ||
				r.virtualHostName.toLowerCase().includes(query)
			);
		})
	);

	function setActiveTab(tab: TabType) {
		activeTab = tab;
	}
</script>

<div class="bg-white rounded-lg border border-gray-200 shadow-sm">
	<!-- Tabs -->
	<div class="border-b border-gray-200">
		<nav class="flex -mb-px">
			<button
				onclick={() => setActiveTab('route-configs')}
				class="px-6 py-3 text-sm font-medium border-b-2 transition-colors
					{activeTab === 'route-configs'
						? 'border-blue-500 text-blue-600'
						: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'}"
			>
				Route Configs
			</button>
			<button
				onclick={() => setActiveTab('virtual-hosts')}
				class="px-6 py-3 text-sm font-medium border-b-2 transition-colors
					{activeTab === 'virtual-hosts'
						? 'border-blue-500 text-blue-600'
						: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'}"
			>
				Virtual Hosts
			</button>
			<button
				onclick={() => setActiveTab('routes')}
				class="px-6 py-3 text-sm font-medium border-b-2 transition-colors
					{activeTab === 'routes'
						? 'border-blue-500 text-blue-600'
						: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'}"
			>
				Routes
			</button>
		</nav>
	</div>

	<!-- Search -->
	<div class="p-4 border-b border-gray-100 bg-gray-50">
		<div class="flex items-center gap-4">
			<div class="flex-1 relative">
				<input
					type="text"
					bind:value={searchQuery}
					placeholder="Search resources..."
					class="w-full pl-10 pr-4 py-2 text-sm border border-gray-300 rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
				/>
				<Search class="absolute left-3 top-2.5 w-4 h-4 text-gray-400" />
			</div>
		</div>
	</div>

	<!-- Route Configs Panel -->
	{#if activeTab === 'route-configs'}
		<div class="p-4">
			<div class="space-y-2">
				{#if filteredRouteConfigs.length === 0}
					<div class="text-center py-8 text-gray-500 text-sm">
						{searchQuery ? 'No route configs found matching your search.' : 'No route configs available.'}
					</div>
				{:else}
					{#each filteredRouteConfigs as rc}
						{#if rc.isAttached}
							<!-- Already attached - disabled checkbox with detach button -->
							<div class="flex items-center gap-3 p-3 bg-blue-50 border border-blue-200 rounded-lg">
								<input type="checkbox" checked disabled class="h-5 w-5 rounded border-gray-300 text-blue-600" />
								<div class="p-1.5 rounded bg-blue-100">
									<Layers class="w-4 h-4 text-blue-600" />
								</div>
								<div class="flex-1">
									<div class="flex items-center gap-2">
										<span class="font-medium text-gray-900">{rc.name}</span>
										<span class="px-2 py-0.5 text-xs rounded-full bg-green-100 text-green-700">Attached</span>
									</div>
									<p class="text-xs text-gray-500 mt-0.5">
										{rc.virtualHostCount} virtual host{rc.virtualHostCount !== 1 ? 's' : ''}, {rc.routeCount} route{rc.routeCount !== 1 ? 's' : ''}
									</p>
								</div>
								<button
									onclick={() => onDetachRouteConfig(rc.name)}
									class="text-xs text-red-600 hover:text-red-800 px-2 py-1 hover:bg-red-50 rounded"
								>
									Detach
								</button>
							</div>
						{:else}
							<!-- Not attached - selectable -->
							<label class="flex items-center gap-3 p-3 border border-gray-200 rounded-lg cursor-pointer hover:bg-gray-50 has-[:checked]:border-blue-500 has-[:checked]:bg-blue-50">
								<input
									type="checkbox"
									checked={selectedRouteConfigs.has(rc.name)}
									onchange={() => onRouteConfigToggle(rc.name)}
									class="h-5 w-5 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
								/>
								<div class="p-1.5 rounded bg-gray-100">
									<Layers class="w-4 h-4 text-gray-600" />
								</div>
								<div class="flex-1">
									<span class="font-medium text-gray-900">{rc.name}</span>
									<p class="text-xs text-gray-500 mt-0.5">
										{rc.virtualHostCount} virtual host{rc.virtualHostCount !== 1 ? 's' : ''}, {rc.routeCount} route{rc.routeCount !== 1 ? 's' : ''}
									</p>
								</div>
							</label>
						{/if}
					{/each}
				{/if}
			</div>
			<p class="text-xs text-gray-500 mt-4 px-1">
				Tip: Attaching to a Route Config applies the filter to ALL routes within that configuration.
			</p>
		</div>
	{/if}

	<!-- Virtual Hosts Panel -->
	{#if activeTab === 'virtual-hosts'}
		<div class="p-4">
			<div class="space-y-2">
				{#if filteredVirtualHosts.length === 0}
					<div class="text-center py-8 text-gray-500 text-sm">
						{searchQuery ? 'No virtual hosts found matching your search.' : 'No virtual hosts available.'}
					</div>
				{:else}
					{#each filteredVirtualHosts as vh}
						{#if vh.isAttached}
							<div class="flex items-center gap-3 p-3 bg-emerald-50 border border-emerald-200 rounded-lg">
								<input type="checkbox" checked disabled class="h-5 w-5 rounded border-gray-300 text-emerald-600" />
								<div class="p-1.5 rounded bg-emerald-100">
									<Globe class="w-4 h-4 text-emerald-600" />
								</div>
								<div class="flex-1">
									<div class="flex items-center gap-2">
										<span class="font-medium text-gray-900">{vh.name}</span>
										<span class="px-2 py-0.5 text-xs rounded-full bg-green-100 text-green-700">Attached</span>
									</div>
									<div class="flex items-center gap-2 mt-0.5">
										<span class="text-xs text-gray-400">in</span>
										<span class="text-xs text-gray-500 font-medium">{vh.routeConfigName}</span>
										<span class="text-xs text-gray-400">|</span>
										<span class="text-xs text-gray-500">{vh.routeCount} route{vh.routeCount !== 1 ? 's' : ''}</span>
									</div>
								</div>
								<button
									onclick={() => onDetachVirtualHost(vh.id)}
									class="text-xs text-red-600 hover:text-red-800 px-2 py-1 hover:bg-red-50 rounded"
								>
									Detach
								</button>
							</div>
						{:else}
							<label class="flex items-center gap-3 p-3 border border-gray-200 rounded-lg cursor-pointer hover:bg-gray-50 has-[:checked]:border-emerald-500 has-[:checked]:bg-emerald-50">
								<input
									type="checkbox"
									checked={selectedVirtualHosts.has(vh.id)}
									onchange={() => onVirtualHostToggle(vh.id)}
									class="h-5 w-5 rounded border-gray-300 text-emerald-600 focus:ring-emerald-500"
								/>
								<div class="p-1.5 rounded bg-gray-100">
									<Globe class="w-4 h-4 text-gray-600" />
								</div>
								<div class="flex-1">
									<span class="font-medium text-gray-900">{vh.name}</span>
									<div class="flex items-center gap-2 mt-0.5">
										<span class="text-xs text-gray-400">in</span>
										<span class="text-xs text-gray-500 font-medium">{vh.routeConfigName}</span>
										<span class="text-xs text-gray-400">|</span>
										<span class="text-xs text-gray-500">{vh.routeCount} route{vh.routeCount !== 1 ? 's' : ''}</span>
									</div>
								</div>
							</label>
						{/if}
					{/each}
				{/if}
			</div>
			<p class="text-xs text-gray-500 mt-4 px-1">
				Tip: Attaching to a Virtual Host applies the filter to all routes in that host.
			</p>
		</div>
	{/if}

	<!-- Routes Panel -->
	{#if activeTab === 'routes'}
		<div class="p-4">
			<div class="space-y-2">
				{#if filteredRoutes.length === 0}
					<div class="text-center py-8 text-gray-500 text-sm">
						{searchQuery ? 'No routes found matching your search.' : 'No routes available.'}
					</div>
				{:else}
					{#each filteredRoutes as route}
						{#if route.isAttached}
							<div class="flex items-center gap-3 p-3 bg-amber-50 border border-amber-200 rounded-lg">
								<input type="checkbox" checked disabled class="h-5 w-5 rounded border-gray-300 text-amber-600" />
								<div class="p-1.5 rounded bg-amber-100">
									<Map class="w-4 h-4 text-amber-600" />
								</div>
								<div class="flex-1">
									<div class="flex items-center gap-2">
										<span class="px-1.5 py-0.5 text-xs font-medium rounded bg-blue-100 text-blue-700">{route.matchType.toUpperCase()}</span>
										<span class="font-medium text-gray-900">{route.name}</span>
										<span class="font-mono text-sm text-gray-500">{route.pathPattern}</span>
										<span class="px-2 py-0.5 text-xs rounded-full bg-green-100 text-green-700">Attached</span>
									</div>
									<div class="flex items-center gap-1 mt-0.5 text-xs text-gray-400">
										<span>{route.routeConfigName}</span>
										<span>/</span>
										<span>{route.virtualHostName}</span>
									</div>
								</div>
								<button
									onclick={() => onDetachRoute(route.id)}
									class="text-xs text-red-600 hover:text-red-800 px-2 py-1 hover:bg-red-50 rounded"
								>
									Detach
								</button>
							</div>
						{:else}
							<label class="flex items-center gap-3 p-3 border border-gray-200 rounded-lg cursor-pointer hover:bg-gray-50 has-[:checked]:border-amber-500 has-[:checked]:bg-amber-50">
								<input
									type="checkbox"
									checked={selectedRoutes.has(route.id)}
									onchange={() => onRouteToggle(route.id)}
									class="h-5 w-5 rounded border-gray-300 text-amber-600 focus:ring-amber-500"
								/>
								<div class="p-1.5 rounded bg-gray-100">
									<Map class="w-4 h-4 text-gray-600" />
								</div>
								<div class="flex-1">
									<div class="flex items-center gap-2">
										<span class="px-1.5 py-0.5 text-xs font-medium rounded bg-blue-100 text-blue-700">{route.matchType.toUpperCase()}</span>
										<span class="font-medium text-gray-900">{route.name}</span>
										<span class="font-mono text-sm text-gray-500">{route.pathPattern}</span>
									</div>
									<div class="flex items-center gap-1 mt-0.5 text-xs text-gray-400">
										<span>{route.routeConfigName}</span>
										<span>/</span>
										<span>{route.virtualHostName}</span>
									</div>
								</div>
							</label>
						{/if}
					{/each}
				{/if}
			</div>
			<p class="text-xs text-gray-500 mt-4 px-1">
				Tip: Route-level filters apply to only that specific route. Use this for granular control.
			</p>
		</div>
	{/if}
</div>
