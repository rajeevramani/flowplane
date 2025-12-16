<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { ArrowLeft, Sliders, Check, Info, AlertTriangle, ChevronDown, ChevronRight, Settings } from 'lucide-svelte';
	import type {
		FilterResponse,
		RouteResponse,
		FilterConfigurationItem,
		FilterInstallationItem,
		ScopeType,
		VirtualHostSummary,
		RouteSummary,
		FilterTypeInfo,
		PerRouteSettings
	} from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import PerRouteSettingsEditor from '$lib/components/filters/PerRouteSettingsEditor.svelte';

	let isLoading = $state(true);
	let isSaving = $state(false);
	let error = $state<string | null>(null);
	let saveError = $state<string | null>(null);
	let saveSuccess = $state<string | null>(null);

	// Data
	let filter = $state<FilterResponse | null>(null);
	let filterTypeInfo = $state<FilterTypeInfo | null>(null);
	let routeConfigs = $state<RouteResponse[]>([]);
	let currentConfigurations = $state<FilterConfigurationItem[]>([]);
	let currentInstallations = $state<FilterInstallationItem[]>([]);

	// Virtual hosts and routes for each route config (loaded on demand)
	let virtualHostsMap = $state<Map<string, VirtualHostSummary[]>>(new Map());
	let routesMap = $state<Map<string, RouteSummary[]>>(new Map());

	// Track expanded state for route configs and vhosts
	let expandedRouteConfigs = $state<Set<string>>(new Set());
	let expandedVhosts = $state<Set<string>>(new Set());

	// Track which scopes are selected for configuration
	let selectedScopes = $state<Set<string>>(new Set()); // Format: "type:id"

	// Track settings per scope - key is "type:id", value is PerRouteSettings
	let scopeSettings = $state<Map<string, PerRouteSettings | null>>(new Map());

	// Track which scope is currently being edited
	let editingScope = $state<string | null>(null);

	// Active tab
	type TabType = 'route-configs' | 'virtual-hosts' | 'routes';
	let activeTab = $state<TabType>('route-configs');

	const filterId = $derived($page.params.id ?? '');

	onMount(async () => {
		if (filterId) {
			await loadData();
		}
	});

	async function loadData() {
		if (!filterId) {
			error = 'Filter ID is required';
			return;
		}

		isLoading = true;
		error = null;

		try {
			// Load filter details
			filter = await apiClient.getFilter(filterId);

			// Load filter type info for per-route behavior and schema
			if (filter) {
				filterTypeInfo = await apiClient.getFilterType(filter.filterType);
			}

			// Load all route configs
			routeConfigs = await apiClient.listRouteConfigs();

			// Load current configurations
			const configsResponse = await apiClient.listFilterConfigurations(filterId);
			currentConfigurations = configsResponse.configurations;

			// Load current installations (to show warning if not installed)
			const installsResponse = await apiClient.listFilterInstallations(filterId);
			currentInstallations = installsResponse.installations;

			// Initialize selection state and settings from current configurations
			const selected = new Set<string>();
			const settings = new Map<string, PerRouteSettings | null>();
			currentConfigurations.forEach((config) => {
				const key = `${config.scopeType}:${config.scopeId}`;
				selected.add(key);
				// Parse settings if present (cast through unknown for type safety)
				if (config.settings) {
					settings.set(key, config.settings as unknown as PerRouteSettings);
				} else {
					settings.set(key, null);
				}
			});
			selectedScopes = selected;
			scopeSettings = settings;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
			console.error('Failed to load data:', e);
		} finally {
			isLoading = false;
		}
	}

	async function loadVirtualHosts(routeConfigName: string) {
		if (virtualHostsMap.has(routeConfigName)) return;

		try {
			const vhosts = await apiClient.listVirtualHosts(routeConfigName);
			const newMap = new Map(virtualHostsMap);
			newMap.set(routeConfigName, vhosts);
			virtualHostsMap = newMap;
		} catch (e) {
			console.error(`Failed to load virtual hosts for ${routeConfigName}:`, e);
		}
	}

	async function loadRoutes(routeConfigName: string, vhostName: string) {
		const key = `${routeConfigName}/${vhostName}`;
		if (routesMap.has(key)) return;

		try {
			const routes = await apiClient.listRoutesInVirtualHost(routeConfigName, vhostName);
			const newMap = new Map(routesMap);
			newMap.set(key, routes);
			routesMap = newMap;
		} catch (e) {
			console.error(`Failed to load routes for ${key}:`, e);
		}
	}

	function toggleRouteConfigExpanded(rcName: string) {
		const newExpanded = new Set(expandedRouteConfigs);
		if (newExpanded.has(rcName)) {
			newExpanded.delete(rcName);
		} else {
			newExpanded.add(rcName);
			loadVirtualHosts(rcName);
		}
		expandedRouteConfigs = newExpanded;
	}

	function toggleVhostExpanded(rcName: string, vhostName: string) {
		const key = `${rcName}/${vhostName}`;
		const newExpanded = new Set(expandedVhosts);
		if (newExpanded.has(key)) {
			newExpanded.delete(key);
		} else {
			newExpanded.add(key);
			loadRoutes(rcName, vhostName);
		}
		expandedVhosts = newExpanded;
	}

	function toggleScope(scopeType: ScopeType, scopeId: string) {
		const key = `${scopeType}:${scopeId}`;
		const newSelected = new Set(selectedScopes);
		if (newSelected.has(key)) {
			newSelected.delete(key);
		} else {
			newSelected.add(key);
		}
		selectedScopes = newSelected;
	}

	function isSelected(scopeType: ScopeType, scopeId: string): boolean {
		return selectedScopes.has(`${scopeType}:${scopeId}`);
	}

	function isCurrentlyConfigured(scopeType: ScopeType, scopeId: string): boolean {
		return currentConfigurations.some(
			(c) => c.scopeType === scopeType && c.scopeId === scopeId
		);
	}

	function getScopeSettings(scopeType: ScopeType, scopeId: string): PerRouteSettings | null {
		return scopeSettings.get(`${scopeType}:${scopeId}`) ?? null;
	}

	function handleSettingsChange(scopeType: ScopeType, scopeId: string, settings: PerRouteSettings | null) {
		const key = `${scopeType}:${scopeId}`;
		const newSettings = new Map(scopeSettings);
		newSettings.set(key, settings);
		scopeSettings = newSettings;
	}

	function toggleEditingScope(scopeType: ScopeType, scopeId: string) {
		const key = `${scopeType}:${scopeId}`;
		if (editingScope === key) {
			editingScope = null;
		} else {
			editingScope = key;
		}
	}

	function getSettingsSummary(settings: PerRouteSettings | null): string {
		if (!settings) return 'Base config';
		switch (settings.behavior) {
			case 'disable':
				return 'Disabled';
			case 'override':
				if (settings.requirementName) {
					return `Requirement: ${settings.requirementName}`;
				}
				return 'Custom override';
			default:
				return 'Base config';
		}
	}

	function getSettingsBadgeVariant(settings: PerRouteSettings | null): 'gray' | 'red' | 'purple' {
		if (!settings) return 'gray';
		switch (settings.behavior) {
			case 'disable':
				return 'red';
			case 'override':
				return 'purple';
			default:
				return 'gray';
		}
	}

	async function handleSave() {
		isSaving = true;
		saveError = null;
		saveSuccess = null;

		try {
			// Determine what changed
			const currentlyConfigured = new Set(
				currentConfigurations.map((c) => `${c.scopeType}:${c.scopeId}`)
			);
			const toConfigure = [...selectedScopes].filter((key) => !currentlyConfigured.has(key));
			const toRemove = [...currentlyConfigured].filter((key) => !selectedScopes.has(key));

			// Also track settings changes for already-configured scopes
			const toUpdate: string[] = [];
			for (const key of selectedScopes) {
				if (currentlyConfigured.has(key)) {
					// Check if settings changed
					const currentConfig = currentConfigurations.find(
						(c) => `${c.scopeType}:${c.scopeId}` === key
					);
					const newSettings = scopeSettings.get(key);
					const currentSettings = currentConfig?.settings as PerRouteSettings | undefined;

					// Compare settings (simple JSON comparison)
					if (JSON.stringify(currentSettings) !== JSON.stringify(newSettings)) {
						toUpdate.push(key);
					}
				}
			}

			// Perform removals
			for (const key of toRemove) {
				const [scopeType, scopeId] = key.split(':') as [ScopeType, string];
				await apiClient.removeFilterConfiguration(filterId, scopeType, scopeId);
			}

			// Perform new configurations
			for (const key of toConfigure) {
				const [scopeType, scopeId] = key.split(':') as [ScopeType, string];
				const settings = scopeSettings.get(key) ?? undefined;
				await apiClient.configureFilter(filterId, {
					scopeType,
					scopeId,
					settings
				});
			}

			// Perform updates (remove and re-add with new settings)
			for (const key of toUpdate) {
				const [scopeType, scopeId] = key.split(':') as [ScopeType, string];
				await apiClient.removeFilterConfiguration(filterId, scopeType, scopeId);
				const settings = scopeSettings.get(key) ?? undefined;
				await apiClient.configureFilter(filterId, {
					scopeType,
					scopeId,
					settings
				});
			}

			const changes = toConfigure.length + toRemove.length + toUpdate.length;
			if (changes > 0) {
				saveSuccess = `Successfully updated configurations. ${toConfigure.length} added, ${toRemove.length} removed, ${toUpdate.length} updated.`;
			} else {
				saveSuccess = 'No changes to save.';
			}

			// Reload to show updated state
			await loadData();
		} catch (e) {
			saveError = e instanceof Error ? e.message : 'Failed to save changes';
		} finally {
			isSaving = false;
		}
	}

	function handleBack() {
		goto('/filters');
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-6">
		<button
			onclick={handleBack}
			class="inline-flex items-center text-sm text-gray-500 hover:text-gray-700 mb-4"
		>
			<ArrowLeft class="h-4 w-4 mr-1" />
			Back to Filters
		</button>

		{#if filter}
			<h1 class="text-3xl font-bold text-gray-900">Configure Filter Scope</h1>
			<p class="mt-2 text-sm text-gray-600">
				Configure which route configs, virtual hosts, or routes the
				<span class="font-semibold">{filter.name}</span> filter should apply to.
			</p>
		{:else}
			<h1 class="text-3xl font-bold text-gray-900">Configure Filter</h1>
		{/if}
	</div>

	<!-- Warning if not installed -->
	{#if !isLoading && currentInstallations.length === 0}
		<div class="bg-yellow-50 border border-yellow-200 rounded-lg p-4 mb-6">
			<div class="flex">
				<AlertTriangle class="h-5 w-5 text-yellow-400 mr-3 flex-shrink-0 mt-0.5" />
				<div class="text-sm text-yellow-700">
					<p class="font-medium mb-1">Filter Not Installed</p>
					<p>
						This filter is not installed on any listeners. Configurations will have no effect
						until the filter is installed. <a
							href={`/filters/${filterId}/install`}
							class="underline font-medium hover:text-yellow-800"
						>Install filter on listeners first</a>.
					</p>
				</div>
			</div>
		</div>
	{/if}

	<!-- Info Banner -->
	<div class="bg-blue-50 border border-blue-200 rounded-lg p-4 mb-6">
		<div class="flex">
			<Info class="h-5 w-5 text-blue-400 mr-3 flex-shrink-0 mt-0.5" />
			<div class="text-sm text-blue-700">
				<p class="font-medium mb-1">What does "Configure" mean?</p>
				<p>
					Configuring a filter for a scope (route config, virtual host, or route) enables per-route
					behavior settings. You can enable/disable the filter for specific routes or apply custom
					settings that override the base configuration.
				</p>
			</div>
		</div>
	</div>

	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading...</span>
			</div>
		</div>
	{:else if error}
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filter}
		<!-- Filter Info Card -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4 mb-6">
			<div class="flex items-center gap-4">
				<div class="p-3 bg-orange-100 rounded-lg">
					<Sliders class="h-6 w-6 text-orange-600" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-gray-900">{filter.name}</h2>
					<div class="flex items-center gap-2 mt-1">
						<Badge variant="blue">{filter.filterType.split('_').map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(' ')}</Badge>
						<span class="text-sm text-gray-500">
							Installed on {currentInstallations.length} listener{currentInstallations.length !== 1 ? 's' : ''}
						</span>
					</div>
				</div>
			</div>
		</div>

		<!-- Save Messages -->
		{#if saveError}
			<div class="bg-red-50 border border-red-200 rounded-md p-4 mb-4">
				<p class="text-sm text-red-800">{saveError}</p>
			</div>
		{/if}

		{#if saveSuccess}
			<div class="bg-green-50 border border-green-200 rounded-md p-4 mb-4">
				<p class="text-sm text-green-800">{saveSuccess}</p>
			</div>
		{/if}

		<!-- Scope Selection -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<div class="px-6 py-4 border-b border-gray-200 bg-gray-50">
				<h3 class="text-lg font-medium text-gray-900">Configure Scopes</h3>
				<p class="text-sm text-gray-500 mt-1">
					Select route configs, virtual hosts, or individual routes to configure this filter for.
				</p>
			</div>

			{#if routeConfigs.length === 0}
				<div class="px-6 py-12 text-center">
					<Sliders class="h-12 w-12 text-gray-400 mx-auto mb-4" />
					<h4 class="text-lg font-medium text-gray-900 mb-2">No Route Configs Available</h4>
					<p class="text-sm text-gray-600">
						Create a route config first before configuring filters.
					</p>
				</div>
			{:else}
				<div class="divide-y divide-gray-200">
					{#each routeConfigs as rc}
						{@const rcSelected = isSelected('route-config', rc.name)}
						{@const rcConfigured = isCurrentlyConfigured('route-config', rc.name)}
						{@const isExpanded = expandedRouteConfigs.has(rc.name)}
						{@const vhosts = virtualHostsMap.get(rc.name) || []}
						{@const rcSettings = getScopeSettings('route-config', rc.name)}
						{@const rcEditKey = `route-config:${rc.name}`}
						{@const isEditingRc = editingScope === rcEditKey}

						<!-- Route Config Row -->
						<div class="bg-white">
							<div
								class="px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
								class:bg-blue-50={rcSelected}
							>
								<div class="flex items-center gap-3">
									<button
										onclick={() => toggleRouteConfigExpanded(rc.name)}
										class="p-1 text-gray-400 hover:text-gray-600"
									>
										{#if isExpanded}
											<ChevronDown class="h-4 w-4" />
										{:else}
											<ChevronRight class="h-4 w-4" />
										{/if}
									</button>

									<button
										onclick={() => toggleScope('route-config', rc.name)}
										class="p-1 rounded-md border-2 transition-colors"
										class:border-blue-500={rcSelected}
										class:bg-blue-500={rcSelected}
										class:border-gray-300={!rcSelected}
									>
										{#if rcSelected}
											<Check class="h-4 w-4 text-white" />
										{:else}
											<div class="h-4 w-4"></div>
										{/if}
									</button>

									<div>
										<div class="flex items-center gap-2">
											<span class="text-sm font-medium text-gray-900">{rc.name}</span>
											<Badge variant="blue">Route Config</Badge>
											{#if rcConfigured}
												<Badge variant="green">Configured</Badge>
											{/if}
											{#if rcSelected && rcSettings}
												<Badge variant={getSettingsBadgeVariant(rcSettings)}>{getSettingsSummary(rcSettings)}</Badge>
											{/if}
										</div>
										<div class="text-xs text-gray-500 mt-0.5">
											Team: {rc.team}
										</div>
									</div>
								</div>
								<!-- Settings button -->
								{#if rcSelected && filterTypeInfo && filterTypeInfo.perRouteBehavior !== 'not_supported'}
									<button
										onclick={() => toggleEditingScope('route-config', rc.name)}
										class="px-3 py-1.5 text-xs font-medium rounded-md transition-colors flex items-center gap-1"
										class:bg-purple-100={isEditingRc}
										class:text-purple-700={isEditingRc}
										class:bg-gray-100={!isEditingRc}
										class:text-gray-600={!isEditingRc}
										class:hover:bg-gray-200={!isEditingRc}
									>
										<Settings class="h-3 w-3" />
										{isEditingRc ? 'Close' : 'Settings'}
									</button>
								{/if}
							</div>

							<!-- Settings Editor Panel for Route Config -->
							{#if isEditingRc && filterTypeInfo}
								<div class="px-6 py-4 bg-gray-50 border-t border-gray-200">
									<PerRouteSettingsEditor
										{filterTypeInfo}
										settings={rcSettings}
										onSettingsChange={(s) => handleSettingsChange('route-config', rc.name, s)}
									/>
								</div>
							{/if}

							<!-- Virtual Hosts (expanded) -->
							{#if isExpanded}
								<div class="pl-12 border-l-2 border-gray-200 ml-6">
									{#each vhosts as vhost}
										{@const vhostScopeId = `${rc.name}/${vhost.name}`}
										{@const vhostSelected = isSelected('virtual-host', vhostScopeId)}
										{@const vhostConfigured = isCurrentlyConfigured('virtual-host', vhostScopeId)}
										{@const vhostExpanded = expandedVhosts.has(vhostScopeId)}
										{@const routes = routesMap.get(vhostScopeId) || []}
										{@const vhostSettings = getScopeSettings('virtual-host', vhostScopeId)}
										{@const vhostEditKey = `virtual-host:${vhostScopeId}`}
										{@const isEditingVhost = editingScope === vhostEditKey}

										<div>
											<div
												class="px-4 py-3 flex items-center justify-between hover:bg-gray-50 transition-colors"
												class:bg-purple-50={vhostSelected}
											>
												<div class="flex items-center gap-3">
													<button
														onclick={() => toggleVhostExpanded(rc.name, vhost.name)}
														class="p-1 text-gray-400 hover:text-gray-600"
													>
														{#if vhostExpanded}
															<ChevronDown class="h-4 w-4" />
														{:else}
															<ChevronRight class="h-4 w-4" />
														{/if}
													</button>

													<button
														onclick={() => toggleScope('virtual-host', vhostScopeId)}
														class="p-1 rounded-md border-2 transition-colors"
														class:border-purple-500={vhostSelected}
														class:bg-purple-500={vhostSelected}
														class:border-gray-300={!vhostSelected}
													>
														{#if vhostSelected}
															<Check class="h-4 w-4 text-white" />
														{:else}
															<div class="h-4 w-4"></div>
														{/if}
													</button>

													<div>
														<div class="flex items-center gap-2">
															<span class="text-sm font-medium text-gray-900">{vhost.name}</span>
															<Badge variant="purple">Virtual Host</Badge>
															{#if vhostConfigured}
																<Badge variant="green">Configured</Badge>
															{/if}
															{#if vhostSelected && vhostSettings}
																<Badge variant={getSettingsBadgeVariant(vhostSettings)}>{getSettingsSummary(vhostSettings)}</Badge>
															{/if}
														</div>
														<div class="text-xs text-gray-500 mt-0.5">
															{vhost.domains.slice(0, 3).join(', ')}{vhost.domains.length > 3 ? '...' : ''}
														</div>
													</div>
												</div>
												<!-- Settings button -->
												{#if vhostSelected && filterTypeInfo && filterTypeInfo.perRouteBehavior !== 'not_supported'}
													<button
														onclick={() => toggleEditingScope('virtual-host', vhostScopeId)}
														class="px-3 py-1.5 text-xs font-medium rounded-md transition-colors flex items-center gap-1"
														class:bg-purple-100={isEditingVhost}
														class:text-purple-700={isEditingVhost}
														class:bg-gray-100={!isEditingVhost}
														class:text-gray-600={!isEditingVhost}
														class:hover:bg-gray-200={!isEditingVhost}
													>
														<Settings class="h-3 w-3" />
														{isEditingVhost ? 'Close' : 'Settings'}
													</button>
												{/if}
											</div>

											<!-- Settings Editor Panel for Virtual Host -->
											{#if isEditingVhost && filterTypeInfo}
												<div class="px-6 py-4 bg-gray-50 border-t border-gray-200 ml-8">
													<PerRouteSettingsEditor
														{filterTypeInfo}
														settings={vhostSettings}
														onSettingsChange={(s) => handleSettingsChange('virtual-host', vhostScopeId, s)}
													/>
												</div>
											{/if}

											<!-- Routes (expanded) -->
											{#if vhostExpanded}
												<div class="pl-12 border-l-2 border-gray-200 ml-4">
													{#each routes as route}
														{@const routeScopeId = `${rc.name}/${vhost.name}/${route.name}`}
														{@const routeSelected = isSelected('route', routeScopeId)}
														{@const routeConfigured = isCurrentlyConfigured('route', routeScopeId)}
														{@const routeSettings = getScopeSettings('route', routeScopeId)}
														{@const routeEditKey = `route:${routeScopeId}`}
														{@const isEditingRoute = editingScope === routeEditKey}

														<div>
															<div
																class="px-4 py-3 flex items-center justify-between hover:bg-gray-50 transition-colors"
																class:bg-orange-50={routeSelected}
															>
																<div class="flex items-center gap-3">
																	<div class="w-6"></div>

																	<button
																		onclick={() => toggleScope('route', routeScopeId)}
																		class="p-1 rounded-md border-2 transition-colors"
																		class:border-orange-500={routeSelected}
																		class:bg-orange-500={routeSelected}
																		class:border-gray-300={!routeSelected}
																	>
																		{#if routeSelected}
																			<Check class="h-4 w-4 text-white" />
																		{:else}
																			<div class="h-4 w-4"></div>
																		{/if}
																	</button>

																	<div>
																		<div class="flex items-center gap-2">
																			<span class="text-sm font-medium text-gray-900">{route.name}</span>
																			<Badge variant="orange">Route</Badge>
																			{#if routeConfigured}
																				<Badge variant="green">Configured</Badge>
																			{/if}
																			{#if routeSelected && routeSettings}
																				<Badge variant={getSettingsBadgeVariant(routeSettings)}>{getSettingsSummary(routeSettings)}</Badge>
																			{/if}
																		</div>
																		<div class="text-xs text-gray-500 mt-0.5">
																			{route.matchType}: {route.pathPattern}
																		</div>
																	</div>
																</div>
																<!-- Settings button -->
																{#if routeSelected && filterTypeInfo && filterTypeInfo.perRouteBehavior !== 'not_supported'}
																	<button
																		onclick={() => toggleEditingScope('route', routeScopeId)}
																		class="px-3 py-1.5 text-xs font-medium rounded-md transition-colors flex items-center gap-1"
																		class:bg-purple-100={isEditingRoute}
																		class:text-purple-700={isEditingRoute}
																		class:bg-gray-100={!isEditingRoute}
																		class:text-gray-600={!isEditingRoute}
																		class:hover:bg-gray-200={!isEditingRoute}
																	>
																		<Settings class="h-3 w-3" />
																		{isEditingRoute ? 'Close' : 'Settings'}
																	</button>
																{/if}
															</div>

															<!-- Settings Editor Panel for Route -->
															{#if isEditingRoute && filterTypeInfo}
																<div class="px-6 py-4 bg-gray-50 border-t border-gray-200 ml-12">
																	<PerRouteSettingsEditor
																		{filterTypeInfo}
																		settings={routeSettings}
																		onSettingsChange={(s) => handleSettingsChange('route', routeScopeId, s)}
																	/>
																</div>
															{/if}
														</div>
													{/each}

													{#if routes.length === 0}
														<div class="px-4 py-3 text-sm text-gray-500 italic">
															No routes in this virtual host
														</div>
													{/if}
												</div>
											{/if}
										</div>
									{/each}

									{#if vhosts.length === 0}
										<div class="px-4 py-3 text-sm text-gray-500 italic">
											Loading virtual hosts...
										</div>
									{/if}
								</div>
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		</div>

		<!-- Action Buttons -->
		<div class="mt-6 flex items-center gap-4">
			<Button onclick={handleSave} variant="primary" disabled={isSaving}>
				{#if isSaving}
					<div class="animate-spin rounded-full h-4 w-4 border-b-2 border-white mr-2"></div>
					Saving...
				{:else}
					Apply Configurations
				{/if}
			</Button>
			<Button onclick={handleBack} variant="secondary">
				Cancel
			</Button>
		</div>
	{/if}
</div>
