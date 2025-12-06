<script lang="ts">
	import type { ListenerResponse, RouteResponse, ClusterResponse } from '$lib/api/types';
	import { apiClient } from '$lib/api/client';

	export interface ListenerFirstConfig {
		selectedTeam: string;
		listenerMode: 'existing' | 'new';
		selectedListenerName: string | null;
		newListenerConfig: { name: string; address: string; port: number };
	}

	interface Props {
		teams: string[];
		config: ListenerFirstConfig;
		onConfigChange: (config: ListenerFirstConfig) => void;
		onRouteConfigLoaded: (routeConfig: RouteResponse | null, clusters: ClusterResponse[]) => void;
		onListenersLoaded?: (listeners: ListenerResponse[]) => void;
	}

	let { teams, config, onConfigChange, onRouteConfigLoaded, onListenersLoaded }: Props = $props();

	let listeners = $state<ListenerResponse[]>([]);
	let isLoadingListeners = $state(false);
	let isLoadingRoutes = $state(false);
	let loadError = $state<string | null>(null);

	// Load listeners when team changes (empty string means "All Teams" for admins)
	$effect(() => {
		if (config.selectedTeam !== null && config.selectedTeam !== undefined) {
			loadListenersForTeam(config.selectedTeam);
		}
	});

	// Load route config when existing listener is selected
	$effect(() => {
		if (config.listenerMode === 'existing' && config.selectedListenerName) {
			loadRouteConfigForListener(config.selectedListenerName);
		} else if (config.listenerMode === 'new') {
			// Clear route config when switching to new listener mode
			onRouteConfigLoaded(null, []);
		}
	});

	async function loadListenersForTeam(team: string) {
		isLoadingListeners = true;
		loadError = null;
		try {
			// Backend filters by user's team scopes automatically:
			// - Admin users get all listeners
			// - Developer users get team-filtered listeners
			const allListeners = await apiClient.listListeners();

			// Filter by selected team for UX (when specific team chosen)
			// When team is empty string (All Teams), show all listeners
			listeners = team ? allListeners.filter((l) => l.team === team) : allListeners;

			onListenersLoaded?.(listeners);
		} catch (e) {
			loadError = e instanceof Error ? e.message : 'Failed to load listeners';
			listeners = [];
		} finally {
			isLoadingListeners = false;
		}
	}

	async function loadRouteConfigForListener(listenerName: string) {
		isLoadingRoutes = true;
		loadError = null;
		try {
			const listener = listeners.find((l) => l.name === listenerName);
			if (!listener) {
				onRouteConfigLoaded(null, []);
				return;
			}

			// Extract route config name from listener's config
			const routeConfigName = extractRouteConfigName(listener.config);
			if (!routeConfigName) {
				onRouteConfigLoaded(null, []);
				return;
			}

			// Fetch the route config
			const routeConfig = await apiClient.getRouteConfig(routeConfigName);

			// Also fetch clusters to populate the cluster selector
			const allClusters = await apiClient.listClusters();
			const teamClusters = allClusters.filter((c) => c.team === config.selectedTeam);

			onRouteConfigLoaded(routeConfig, teamClusters);
		} catch (e) {
			// Route config might not exist, that's okay
			console.warn('Could not load route config:', e);
			onRouteConfigLoaded(null, []);
		} finally {
			isLoadingRoutes = false;
		}
	}

	function extractRouteConfigName(listenerConfig: ListenerResponse['config']): string | null {
		try {
			const filterChains = listenerConfig?.filter_chains;
			if (!filterChains || filterChains.length === 0) return null;

			for (const chain of filterChains) {
				for (const filter of chain.filters || []) {
					const filterType = filter.filter_type;
					if (filterType?.HttpConnectionManager?.route_config_name) {
						return filterType.HttpConnectionManager.route_config_name;
					}
				}
			}
			return null;
		} catch {
			return null;
		}
	}

	function handleTeamChange(e: Event) {
		const target = e.target as HTMLSelectElement;
		onConfigChange({
			...config,
			selectedTeam: target.value,
			selectedListenerName: null // Reset listener when team changes
		});
	}

	function handleModeChange(mode: 'existing' | 'new') {
		onConfigChange({
			...config,
			listenerMode: mode,
			selectedListenerName: mode === 'existing' ? config.selectedListenerName : null
		});
	}

	function handleListenerChange(e: Event) {
		const target = e.target as HTMLSelectElement;
		onConfigChange({
			...config,
			selectedListenerName: target.value || null
		});
	}

	function handleNewListenerChange(field: 'name' | 'address' | 'port', value: string | number) {
		onConfigChange({
			...config,
			newListenerConfig: {
				...config.newListenerConfig,
				[field]: value
			}
		});
	}
</script>

<div class="space-y-6">
	<h3 class="text-lg font-medium text-gray-900">Step 1: Select Listener</h3>

	<div class="bg-gray-50 rounded-lg p-4 space-y-4">
		<!-- Team Selector -->
		<div>
			<label for="team-select" class="block text-sm font-medium text-gray-700 mb-1">Team</label>
			{#if teams.length > 1}
				<select
					id="team-select"
					value={config.selectedTeam}
					onchange={handleTeamChange}
					class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
				>
					<option value="">Select a team...</option>
					{#each teams as team}
						<option value={team}>{team === '' ? 'All Teams' : team}</option>
					{/each}
				</select>
			{:else if teams.length === 1}
				<input
					type="text"
					value={teams[0] === '' ? 'All Teams' : config.selectedTeam}
					readonly
					class="w-full rounded-md border border-gray-300 bg-gray-100 px-3 py-2 text-sm"
				/>
			{:else}
				<p class="text-sm text-gray-500">No teams available</p>
			{/if}
		</div>

		{#if config.selectedTeam !== null && config.selectedTeam !== undefined}
			<!-- Listener Mode Selection -->
			<div class="space-y-3">
				<label class="flex items-center gap-3 cursor-pointer">
					<input
						type="radio"
						name="listener-mode"
						checked={config.listenerMode === 'existing'}
						onchange={() => handleModeChange('existing')}
						class="h-4 w-4 text-blue-600 focus:ring-blue-500"
					/>
					<span class="text-sm text-gray-700">Use existing listener</span>
				</label>

				{#if config.listenerMode === 'existing'}
					<div class="ml-7">
						{#if isLoadingListeners}
							<div class="flex items-center gap-2 text-sm text-gray-500">
								<svg class="animate-spin h-4 w-4" fill="none" viewBox="0 0 24 24">
									<circle
										class="opacity-25"
										cx="12"
										cy="12"
										r="10"
										stroke="currentColor"
										stroke-width="4"
									></circle>
									<path
										class="opacity-75"
										fill="currentColor"
										d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"
									></path>
								</svg>
								Loading listeners...
							</div>
						{:else if listeners.length === 0}
							<p class="text-sm text-gray-500 italic">No existing listeners available for this team</p>
						{:else}
							<select
								value={config.selectedListenerName ?? ''}
								onchange={handleListenerChange}
								class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
							>
								<option value="">Select a listener...</option>
								{#each listeners as listener}
									<option value={listener.name}>
										{listener.name} ({listener.address}:{listener.port})
									</option>
								{/each}
							</select>
						{/if}

						{#if isLoadingRoutes}
							<div class="mt-2 flex items-center gap-2 text-sm text-gray-500">
								<svg class="animate-spin h-4 w-4" fill="none" viewBox="0 0 24 24">
									<circle
										class="opacity-25"
										cx="12"
										cy="12"
										r="10"
										stroke="currentColor"
										stroke-width="4"
									></circle>
									<path
										class="opacity-75"
										fill="currentColor"
										d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"
									></path>
								</svg>
								Loading route configuration...
							</div>
						{/if}
					</div>
				{/if}

				<label class="flex items-center gap-3 cursor-pointer">
					<input
						type="radio"
						name="listener-mode"
						checked={config.listenerMode === 'new'}
						onchange={() => handleModeChange('new')}
						class="h-4 w-4 text-blue-600 focus:ring-blue-500"
					/>
					<span class="text-sm text-gray-700">Create new listener</span>
				</label>

				{#if config.listenerMode === 'new'}
					<div class="ml-7 space-y-3">
						<div>
							<label for="new-listener-name" class="block text-xs text-gray-500 mb-1"
								>Listener Name</label
							>
							<input
								id="new-listener-name"
								type="text"
								placeholder="my-api-listener"
								value={config.newListenerConfig.name}
								oninput={(e) =>
									handleNewListenerChange('name', (e.target as HTMLInputElement).value)}
								class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
							/>
						</div>
						<div class="flex items-center gap-3">
							<div class="flex-1">
								<label for="new-listener-address" class="block text-xs text-gray-500 mb-1"
									>Address</label
								>
								<input
									id="new-listener-address"
									type="text"
									placeholder="0.0.0.0"
									value={config.newListenerConfig.address}
									oninput={(e) =>
										handleNewListenerChange('address', (e.target as HTMLInputElement).value)}
									class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
								/>
							</div>
							<div class="w-32">
								<label for="new-listener-port" class="block text-xs text-gray-500 mb-1">Port</label>
								<input
									id="new-listener-port"
									type="number"
									min="1024"
									max="65535"
									placeholder="8080"
									value={config.newListenerConfig.port}
									oninput={(e) =>
										handleNewListenerChange('port', Number((e.target as HTMLInputElement).value))}
									class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
								/>
							</div>
						</div>
					</div>
				{/if}
			</div>
		{/if}

		{#if loadError}
			<div class="text-sm text-red-600">{loadError}</div>
		{/if}
	</div>
</div>
