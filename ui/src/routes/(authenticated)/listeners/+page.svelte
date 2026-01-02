<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Plus, Edit, Trash2, Radio, Lock, Shield, Route } from 'lucide-svelte';
	import type { ListenerResponse, RouteResponse, ImportSummary } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');

	// Data
	let listeners = $state<ListenerResponse[]>([]);
	let routes = $state<RouteResponse[]>([]);
	let imports = $state<ImportSummary[]>([]);

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
			const [listenersData, routesData, importsData] = await Promise.all([
				apiClient.listListeners(),
				apiClient.listRouteConfigs(),
				currentTeam ? apiClient.listImports(currentTeam) : Promise.resolve([])
			]);

			listeners = listenersData;
			routes = routesData;
			imports = importsData;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Get routes for a listener
	function getRoutesForListener(listener: ListenerResponse): RouteResponse[] {
		const routeNames = new Set<string>();

		listener.config?.filter_chains?.forEach(
			(fc: { filters?: { filter_type?: { HttpConnectionManager?: { route_config_name?: string } } }[] }) => {
				fc.filters?.forEach((f) => {
					const routeConfigName = f.filter_type?.HttpConnectionManager?.route_config_name;
					if (routeConfigName) {
						routeNames.add(routeConfigName);
					}
				});
			}
		);

		return routes.filter((r) => routeNames.has(r.name));
	}

	// Calculate stats
	let stats = $derived({
		totalListeners: listeners.length,
		httpListeners: listeners.filter(l => (l.protocol || 'HTTP') === 'HTTP').length,
		httpsListeners: listeners.filter(l => {
			const config = l.config || {};
			return config.filter_chains?.some(
				(fc: { tls_context?: unknown }) => fc.tls_context
			);
		}).length,
		totalRoutes: listeners.reduce((sum, listener) => {
			return sum + getRoutesForListener(listener).length;
		}, 0)
	});

	// Filter listeners
	let filteredListeners = $derived(
		listeners
			.filter(listener => listener.team === currentTeam)
			.filter(listener =>
				!searchQuery ||
				listener.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				listener.address.toLowerCase().includes(searchQuery.toLowerCase())
			)
	);

	// Get listener features
	function getListenerFeatures(listener: ListenerResponse) {
		const config = listener.config || {};
		return {
			hasTls: config.filter_chains?.some((fc: { tls_context?: unknown }) => fc.tls_context),
			routeCount: getRoutesForListener(listener).length
		};
	}

	// Get source type
	function getSourceType(listener: ListenerResponse): { type: string; name?: string } {
		if (listener.importId) {
			const importRecord = imports.find(i => i.id === listener.importId);
			return {
				type: 'import',
				name: importRecord?.specName || 'OpenAPI Import'
			};
		}
		return { type: 'manual', name: 'Manual' };
	}

	// Get route config names from listener
	function getRouteConfigNames(listener: ListenerResponse): string[] {
		const names: string[] = [];
		const config = listener.config || {};
		const filterChains = config.filter_chains || [];

		for (const fc of filterChains as any[]) {
			const filters = fc.filters || [];
			for (const filter of filters) {
				if (filter.filter_type?.HttpConnectionManager?.route_config_name) {
					names.push(filter.filter_type.HttpConnectionManager.route_config_name);
				}
			}
		}

		return [...new Set(names)]; // Remove duplicates
	}

	// Navigate to create page
	function handleCreate() {
		goto('/listeners/create');
	}

	// Navigate to edit page
	function handleEdit(listenerName: string) {
		goto(`/listeners/${encodeURIComponent(listenerName)}/edit`);
	}

	// Delete listener
	async function handleDelete(listener: ListenerResponse) {
		if (!confirm(`Are you sure you want to delete the listener "${listener.name}"? This action cannot be undone.`)) {
			return;
		}

		try {
			await apiClient.deleteListener(listener.name);
			await loadData();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete listener';
		}
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Listeners</h1>
		<p class="mt-2 text-sm text-gray-600">
			Manage network listeners for the <span class="font-medium">{currentTeam}</span> team
		</p>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6">
		<Button onclick={handleCreate} variant="primary">
			<Plus class="h-4 w-4 mr-2" />
			Create Listener
		</Button>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-4 gap-4 mb-6">
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Listeners</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalListeners}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<Radio class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">HTTP Listeners</p>
					<p class="text-2xl font-bold text-gray-900">{stats.httpListeners}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<Radio class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">HTTPS Listeners</p>
					<p class="text-2xl font-bold text-gray-900">{stats.httpsListeners}</p>
				</div>
				<div class="p-3 bg-purple-100 rounded-lg">
					<Lock class="h-6 w-6 text-purple-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Routes</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalRoutes}</p>
				</div>
				<div class="p-3 bg-orange-100 rounded-lg">
					<Route class="h-6 w-6 text-orange-600" />
				</div>
			</div>
		</div>
	</div>

	<!-- Search -->
	<div class="mb-6">
		<input
			type="text"
			bind:value={searchQuery}
			placeholder="Search by name or address..."
			class="w-full md:w-96 px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		/>
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading listeners...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filteredListeners.length === 0}
		<!-- Empty State -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
			<Radio class="h-12 w-12 text-gray-400 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">
				{searchQuery ? 'No listeners found' : 'No listeners yet'}
			</h3>
			<p class="text-sm text-gray-600 mb-6">
				{searchQuery
					? 'Try adjusting your search query'
					: 'Get started by creating a new listener'}
			</p>
			{#if !searchQuery}
				<Button onclick={handleCreate} variant="primary">
					<Plus class="h-4 w-4 mr-2" />
					Create Listener
				</Button>
			{/if}
		</div>
	{:else}
		<!-- Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-x-auto">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Name
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Team
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Address
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Protocol
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							TLS
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Route Configs
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Source
						</th>
						<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
							Actions
						</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each filteredListeners as listener}
						{@const features = getListenerFeatures(listener)}
						{@const source = getSourceType(listener)}
						<tr class="hover:bg-gray-50 transition-colors">
							<!-- Name -->
							<td class="px-6 py-4">
								<span class="text-sm font-medium text-gray-900">{listener.name}</span>
							</td>

							<!-- Team -->
							<td class="px-6 py-4">
								<Badge variant="indigo">{listener.team}</Badge>
							</td>

							<!-- Address -->
							<td class="px-6 py-4">
								<span class="text-sm text-gray-900 font-mono">{listener.address}:{listener.port}</span>
							</td>

							<!-- Protocol -->
							<td class="px-6 py-4">
								<Badge variant="gray">{listener.protocol || 'HTTP'}</Badge>
							</td>

							<!-- TLS -->
							<td class="px-6 py-4">
								{#if features.hasTls}
									<Badge variant="green">Enabled</Badge>
								{:else}
									<Badge variant="gray">Disabled</Badge>
								{/if}
							</td>

							<!-- Route Configs -->
							<td class="px-6 py-4">
								{#if getRouteConfigNames(listener).length > 0}
									{@const routeConfigNames = getRouteConfigNames(listener)}
									<div class="flex flex-wrap gap-1">
										{#each routeConfigNames as routeConfigName}
											<Badge variant="blue">{routeConfigName}</Badge>
										{/each}
									</div>
								{:else}
									<span class="text-sm text-gray-500">None</span>
								{/if}
							</td>

							<!-- Source -->
							<td class="px-6 py-4">
								{#if source.type === 'import'}
									<Badge variant="purple">{source.name}</Badge>
								{:else}
									<span class="text-sm text-gray-500">{source.name}</span>
								{/if}
							</td>

							<!-- Actions -->
							<td class="px-6 py-4 text-right">
								<div class="flex justify-end gap-2">
									<button
										onclick={() => handleEdit(listener.name)}
										class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
										title="Edit listener"
									>
										<Edit class="h-4 w-4" />
									</button>
									<button
										onclick={() => handleDelete(listener)}
										class="p-2 text-red-600 hover:bg-red-50 rounded-md transition-colors"
										title="Delete listener"
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

		<!-- Pagination Placeholder -->
		{#if filteredListeners.length > 50}
			<div class="mt-4 flex justify-center">
				<p class="text-sm text-gray-600">Showing {filteredListeners.length} listeners</p>
			</div>
		{/if}
	{/if}
</div>
