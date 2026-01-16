<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { listRouteViews } from '$lib/api/route-views';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { Plus, ChevronDown, ChevronUp, ArrowLeft, Filter, Zap } from 'lucide-svelte';
	import type { ClusterResponse, RouteResponse, CreateRouteBody, FilterResponse, HierarchicalFilterContext, McpStatus, McpSchemaSource, EnableMcpRequest } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import VirtualHostEditor, {
		type VirtualHostFormState,
		type RouteFormState
	} from '$lib/components/route-config/VirtualHostEditor.svelte';
	import JsonPanel from '$lib/components/route-config/JsonPanel.svelte';
	import Button from '$lib/components/Button.svelte';
	import FilterAttachmentList from '$lib/components/filters/FilterAttachmentList.svelte';
	import FilterSelectorModal from '$lib/components/filters/FilterSelectorModal.svelte';

	interface FormState {
		name: string;
		team: string;
		virtualHosts: VirtualHostFormState[];
	}

	let currentTeam = $state<string>('');
	let configId = $derived($page.params.id);
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let isSubmitting = $state(false);
	let clusters = $state<ClusterResponse[]>([]);
	let advancedExpanded = $state(false);
	let originalConfig = $state<RouteResponse | null>(null);
	let activeTab = $state<'configuration' | 'json'>('configuration');

	// Filter attachment state (config-level)
	let attachedFilters = $state<FilterResponse[]>([]);
	let availableFilters = $state<FilterResponse[]>([]);
	let isLoadingFilters = $state(false);
	let filtersExpanded = $state(true);
	let showFilterModal = $state(false);
	let filterError = $state<string | null>(null);

	// Hierarchical filter state
	let virtualHostFiltersMap = $state<Map<string, FilterResponse[]>>(new Map());
	let routeFiltersMap = $state<Map<string, Map<string, FilterResponse[]>>>(new Map()); // vhName -> (routeName -> filters)
	let currentFilterContext = $state<HierarchicalFilterContext | null>(null);

	// MCP configuration state
	let mcpExpanded = $state(false);
	let mcpStatus = $state<McpStatus | null>(null);
	let isLoadingMcp = $state(false);
	let mcpError = $state<string | null>(null);
	let isSavingMcp = $state(false);
	let routeIds = $state<string[]>([]);
	let mcpEnabledCount = $state(0);

	// MCP form state
	let mcpEnabled = $state(false);
	let mcpToolName = $state('');
	let mcpDescription = $state('');
	let mcpSchemaSource = $state<McpSchemaSource>('openapi');

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Initialize empty form state
	let formState = $state<FormState>({
		name: '',
		team: currentTeam,
		virtualHosts: []
	});

	// Load data
	onMount(async () => {
		await loadData();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		if (!configId) {
			error = 'Configuration ID is required';
			isLoading = false;
			return;
		}

		try {
			const [config, clustersData] = await Promise.all([
				apiClient.getRouteConfig(configId),
				apiClient.listClusters()
			]);

			originalConfig = config;
			clusters = clustersData;

			// Parse config into form state
			formState = parseRouteConfigToForm(config);

			// Load filters and MCP status (separate from main data load to not block UI)
			loadFilters();
			loadMcpStatus();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load configuration';
		} finally {
			isLoading = false;
		}
	}

	async function loadFilters() {
		if (!configId) {
			console.warn('loadFilters called without configId');
			return;
		}

		isLoadingFilters = true;
		filterError = null;

		try {
			// Load config-level filters and all available filters in parallel
			const [routeFiltersResponse, allFilters] = await Promise.all([
				apiClient.listRouteConfigFilters(configId),
				apiClient.listFilters()
			]);

			attachedFilters = routeFiltersResponse.filters;
			availableFilters = allFilters;

			// Also load hierarchical filters for each virtual host and route
			await loadHierarchicalFilters();

			console.debug('Loaded filters:', {
				attached: attachedFilters.length,
				available: availableFilters.length,
				availableTypes: availableFilters.map(f => f.filterType)
			});
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to load filters';
			console.error('Failed to load filters:', e);
			// Ensure arrays are reset on error
			attachedFilters = [];
			availableFilters = [];
		} finally {
			isLoadingFilters = false;
		}
	}

	async function loadMcpStatus() {
		if (!configId || !currentTeam) {
			console.warn('loadMcpStatus called without configId or team');
			return;
		}

		isLoadingMcp = true;
		mcpError = null;

		try {
			// Fetch all route IDs for this route config using route-views API
			const routeViewsResponse = await listRouteViews({
				routeConfig: configId,
				pageSize: 100 // Get up to 100 routes
			});

			routeIds = routeViewsResponse.items.map(item => item.routeId);

			if (routeIds.length === 0) {
				mcpError = 'No routes found for this configuration';
				mcpStatus = null;
				mcpEnabled = false;
				mcpEnabledCount = 0;
				return;
			}

			// Count how many routes have MCP enabled
			mcpEnabledCount = routeViewsResponse.items.filter(item => item.mcpEnabled).length;

			// Get MCP status from the first route
			const status = await apiClient.getMcpStatus(currentTeam, routeIds[0]);
			mcpStatus = status;

			// Set enabled if any routes have MCP enabled
			mcpEnabled = mcpEnabledCount > 0;
			mcpToolName = status.toolName || '';
			mcpDescription = status.metadata?.description || status.metadata?.summary || '';
			mcpSchemaSource = (status.recommendedSource as McpSchemaSource) || 'openapi';

			console.debug('Loaded MCP status:', {
				routeCount: routeIds.length,
				mcpEnabledCount,
				status
			});
		} catch (e) {
			// MCP may not be available for this route - that's okay
			mcpError = e instanceof Error ? e.message : 'Failed to load MCP status';
			console.debug('MCP not available:', e);
			mcpStatus = null;
			mcpEnabled = false;
			mcpEnabledCount = 0;
		} finally {
			isLoadingMcp = false;
		}
	}

	async function loadHierarchicalFilters() {
		if (!configId) return;

		const newVhFiltersMap = new Map<string, FilterResponse[]>();
		const newRouteFiltersMap = new Map<string, Map<string, FilterResponse[]>>();

		// Load filters for each virtual host
		for (const vh of formState.virtualHosts) {
			try {
				const vhFiltersResponse = await apiClient.listVirtualHostFilters(configId, vh.name);
				newVhFiltersMap.set(vh.name, vhFiltersResponse.filters);

				// Load filters for each route in this virtual host
				const routeMap = new Map<string, FilterResponse[]>();
				for (const route of vh.routes) {
					try {
						const routeFiltersResponse = await apiClient.listRouteHierarchyFilters(configId, vh.name, route.name);
						routeMap.set(route.name, routeFiltersResponse.filters);
					} catch (e) {
						console.debug(`No route filters for ${vh.name}/${route.name}:`, e);
						routeMap.set(route.name, []);
					}
				}
				newRouteFiltersMap.set(vh.name, routeMap);
			} catch (e) {
				console.debug(`No VH filters for ${vh.name}:`, e);
				newVhFiltersMap.set(vh.name, []);
				newRouteFiltersMap.set(vh.name, new Map());
			}
		}

		virtualHostFiltersMap = newVhFiltersMap;
		routeFiltersMap = newRouteFiltersMap;
	}

	// Config-level filter handlers
	async function handleConfigureConfigFilter(filterId: string, _order?: number) {
		if (!configId) return;

		try {
			await apiClient.configureFilter(filterId, {
				scopeType: 'route-config',
				scopeId: configId
			});
			await loadFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to configure filter';
			console.error('Failed to configure filter:', e);
		}
	}

	async function handleRemoveConfigFilter(filterId: string) {
		if (!configId) return;

		try {
			await apiClient.removeFilterConfiguration(filterId, 'route-config', configId);
			await loadFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to remove filter configuration';
			console.error('Failed to remove filter configuration:', e);
		}
	}

	// Virtual Host filter handlers
	function handleOpenVirtualHostFilterModal(virtualHostName: string) {
		currentFilterContext = {
			level: 'virtual_host',
			routeConfigName: configId!,
			virtualHostName
		};
		console.debug('Opening VH filter modal:', {
			context: currentFilterContext,
			availableFiltersCount: availableFilters.length,
			attachedFilterIds: attachedFilterIds()
		});
		showFilterModal = true;
	}

	async function handleConfigureVirtualHostFilter(virtualHostName: string, filterId: string, _order?: number) {
		if (!configId) return;

		try {
			await apiClient.configureFilter(filterId, {
				scopeType: 'virtual-host',
				scopeId: `${configId}/${virtualHostName}`
			});
			await loadHierarchicalFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to configure filter for virtual host';
			console.error('Failed to configure filter for virtual host:', e);
		}
	}

	async function handleRemoveVirtualHostFilter(virtualHostName: string, filterId: string) {
		if (!configId) return;

		try {
			await apiClient.removeFilterConfiguration(filterId, 'virtual-host', `${configId}/${virtualHostName}`);
			await loadHierarchicalFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to remove filter configuration from virtual host';
			console.error('Failed to remove filter configuration from virtual host:', e);
		}
	}

	// Route filter handlers
	function handleOpenRouteFilterModal(virtualHostName: string, routeName: string) {
		currentFilterContext = {
			level: 'route',
			routeConfigName: configId!,
			virtualHostName,
			routeName
		};
		console.debug('Opening route filter modal:', {
			context: currentFilterContext,
			availableFiltersCount: availableFilters.length,
			attachedFilterIds: attachedFilterIds()
		});
		showFilterModal = true;
	}

	async function handleConfigureRouteFilter(virtualHostName: string, routeName: string, filterId: string, _order?: number) {
		if (!configId) return;

		try {
			await apiClient.configureFilter(filterId, {
				scopeType: 'route',
				scopeId: `${configId}/${virtualHostName}/${routeName}`
			});
			await loadHierarchicalFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to configure filter for route';
			console.error('Failed to configure filter for route:', e);
		}
	}

	async function handleRemoveRouteFilter(virtualHostName: string, routeName: string, filterId: string) {
		if (!configId) return;

		try {
			await apiClient.removeFilterConfiguration(filterId, 'route', `${configId}/${virtualHostName}/${routeName}`);
			await loadHierarchicalFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to remove filter configuration from route';
			console.error('Failed to remove filter configuration from route:', e);
		}
	}

	// Universal filter modal handler
	async function handleFilterModalSelect(filterId: string, order?: number) {
		if (!currentFilterContext) return;

		switch (currentFilterContext.level) {
			case 'route_config':
				await handleConfigureConfigFilter(filterId, order);
				break;
			case 'virtual_host':
				if (currentFilterContext.virtualHostName) {
					await handleConfigureVirtualHostFilter(currentFilterContext.virtualHostName, filterId, order);
				}
				break;
			case 'route':
				if (currentFilterContext.virtualHostName && currentFilterContext.routeName) {
					await handleConfigureRouteFilter(
						currentFilterContext.virtualHostName,
						currentFilterContext.routeName,
						filterId,
						order
					);
				}
				break;
		}
	}

	// Derived: IDs of already attached filters based on current context
	let attachedFilterIds = $derived(() => {
		if (!currentFilterContext) {
			return attachedFilters.map((f) => f.id);
		}

		switch (currentFilterContext.level) {
			case 'route_config':
				return attachedFilters.map((f) => f.id);
			case 'virtual_host':
				const vhFilters = virtualHostFiltersMap.get(currentFilterContext.virtualHostName || '') || [];
				return vhFilters.map((f) => f.id);
			case 'route':
				const routeMap = routeFiltersMap.get(currentFilterContext.virtualHostName || '');
				const routeFilters = routeMap?.get(currentFilterContext.routeName || '') || [];
				return routeFilters.map((f) => f.id);
			default:
				return attachedFilters.map((f) => f.id);
		}
	});

	// Parse RouteResponse to form state
	function parseRouteConfigToForm(config: RouteResponse): FormState {
		console.log('Parsing route config:', config);

		// The config.config contains the RouteDefinitionDto with virtualHosts
		const routeConfig = config.config as any;
		const virtualHosts = routeConfig?.virtualHosts || [];

		return {
			name: config.name || '',
			team: config.team || currentTeam,
			virtualHosts: virtualHosts.map((vh: any, vhIndex: number) => {
				console.log(`Virtual Host ${vhIndex}:`, vh);

				return {
					id: `vh-${vhIndex}-${Date.now()}`,
					name: vh.name || `vhost-${vhIndex + 1}`,
					domains: vh.domains || [],
					routes: (vh.routes || []).map((route: any, routeIndex: number) => {
						console.log(`Route ${vhIndex}-${routeIndex}:`, route);
						console.log('Route match:', route.match);
						console.log('Route path:', route.match?.path);

						const method = route.match?.headers?.find((h: any) => h.name === ':method')?.value || 'GET';
						const action = route.action || {};
						const retryPolicy = action.retryPolicy;
						const pathObj = route.match?.path;
						const pathType = pathObj?.type || 'prefix';

						// Extract path - template type uses 'template' field, others (exact/prefix/regex) use 'value'
						const path = pathObj?.template || pathObj?.value || '/';

						console.log(`Extracted path: ${path}, type: ${pathType}`);

						return {
							id: `route-${vhIndex}-${routeIndex}-${Date.now()}`,
							name: route.name || `route-${routeIndex}`,
							method: method,
							path: path,
							pathType: pathType,
							cluster: action.type === 'forward' ? action.cluster : '',
							timeout: action.timeoutSeconds || 30,
							// Path rewrites
							prefixRewrite: action.prefixRewrite,
							templateRewrite: action.templateRewrite,
							// Retry policy
							retryEnabled: !!retryPolicy,
							maxRetries: retryPolicy?.maxRetries,
							retryOn: retryPolicy?.retryOn,
							perTryTimeout: retryPolicy?.perTryTimeoutSeconds,
							backoffBaseMs: retryPolicy?.backoff?.baseIntervalMs,
							backoffMaxMs: retryPolicy?.backoff?.maxIntervalMs
						};
					})
				};
			})
		};
	}

	// Get available cluster names
	let availableClusters = $derived(clusters.map((c) => c.name));

	// Build JSON payload from form state
	let jsonPayload = $derived(buildRouteConfigJSON(formState));

	function buildRouteConfigJSON(form: FormState): string {
		const payload: any = {
			team: form.team || currentTeam,
			name: form.name || '',
			virtualHosts: form.virtualHosts.map((vh) => ({
				name: vh.name,
				domains: vh.domains,
				routes: vh.routes.map((r) => {
					const action: any = {
						type: 'forward' as const,
						cluster: r.cluster,
						timeoutSeconds: r.timeout || 30
					};

					// Add path rewrites if specified
					if (r.prefixRewrite) {
						action.prefixRewrite = r.prefixRewrite;
					}
					if (r.templateRewrite) {
						action.templateRewrite = r.templateRewrite;
					}

					// Add retry policy if enabled
					if (r.retryEnabled) {
						action.retryPolicy = {
							maxRetries: r.maxRetries || 3,
							retryOn: r.retryOn || ['5xx', 'reset', 'connect-failure'],
							perTryTimeoutSeconds: r.perTryTimeout || 10,
							backoff: {
								baseIntervalMs: r.backoffBaseMs || 100,
								maxIntervalMs: r.backoffMaxMs || 1000
							}
						};
					}

					return {
						name: r.name,
						match: {
							path: r.pathType === 'template'
								? { type: r.pathType, template: r.path }
								: { type: r.pathType, value: r.path },
							headers: [
								{
									name: ':method',
									value: r.method
								}
							]
						},
						action
					};
				})
			}))
		};

		return JSON.stringify(payload, null, 2);
	}

	// Add virtual host
	function handleAddVirtualHost() {
		const vhNumber = formState.virtualHosts.length + 1;
		formState.virtualHosts = [
			...formState.virtualHosts,
			{
				id: `vh-${Date.now()}`,
				name: `vhost-${vhNumber}`,
				domains: [],
				routes: []
			}
		];
	}

	// Remove virtual host
	function handleRemoveVirtualHost(index: number) {
		formState.virtualHosts = formState.virtualHosts.filter((_, i) => i !== index);
	}

	// Update virtual host
	function handleUpdateVirtualHost(index: number, updated: VirtualHostFormState) {
		formState.virtualHosts = formState.virtualHosts.map((vh, i) =>
			i === index ? updated : vh
		);
	}

	// Validate form
	function validateForm(): string | null {
		if (!formState.name) return 'Configuration name is required';
		if (!/^[a-z0-9-]+$/.test(formState.name))
			return 'Name must be lowercase alphanumeric with dashes';
		if (formState.virtualHosts.length === 0) return 'At least one virtual host is required';

		for (const vh of formState.virtualHosts) {
			if (vh.domains.length === 0) return `Virtual host "${vh.name}" must have at least one domain`;
			if (vh.routes.length === 0) return `Virtual host "${vh.name}" must have at least one route`;
		}

		return null;
	}

	// Handle form submission
	async function handleSubmit() {
		error = null;
		const validationError = validateForm();
		if (validationError) {
			error = validationError;
			return;
		}

		isSubmitting = true;

		try {
			const payload = JSON.parse(jsonPayload);
			console.log('Submitting payload:', payload);
			await apiClient.updateRouteConfig(configId!, payload);
			goto('/route-configs');
		} catch (e) {
			console.error('Update failed:', e);
			// Extract detailed error message if available
			if (e && typeof e === 'object' && 'message' in e) {
				error = (e as any).message;
			} else {
				error = 'Failed to update configuration';
			}
		} finally {
			isSubmitting = false;
		}
	}

	// Handle cancel
	function handleCancel() {
		goto('/route-configs');
	}

	// Open config-level filter modal
	function handleOpenConfigFilterModal() {
		currentFilterContext = {
			level: 'route_config',
			routeConfigName: configId!
		};
		console.debug('Opening filter modal:', {
			context: currentFilterContext,
			availableFiltersCount: availableFilters.length,
			attachedFilterIds: attachedFilterIds(),
			availableFilterTypes: availableFilters.map(f => f.filterType)
		});
		showFilterModal = true;
	}

	// MCP handlers
	async function handleToggleMcp() {
		if (!currentTeam || routeIds.length === 0) {
			mcpError = 'No routes found to toggle MCP';
			return;
		}

		mcpError = null;
		isSavingMcp = true;

		try {
			if (mcpEnabled) {
				// Disable MCP on all routes
				await apiClient.bulkDisableMcp(currentTeam, { routeIds });
				mcpEnabled = false;
				mcpEnabledCount = 0;
			} else {
				// Enable MCP on all routes (tool names and metadata are auto-generated)
				await apiClient.bulkEnableMcp(currentTeam, { routeIds });
				mcpEnabled = true;
				mcpEnabledCount = routeIds.length;
			}

			// Reload MCP status to get updated state
			await loadMcpStatus();
		} catch (e) {
			mcpError = e instanceof Error ? e.message : 'Failed to toggle MCP';
			console.error('Failed to toggle MCP:', e);
		} finally {
			isSavingMcp = false;
		}
	}

	async function handleSaveMcpConfig() {
		if (!currentTeam || !mcpEnabled || routeIds.length === 0) return;

		mcpError = null;
		isSavingMcp = true;

		try {
			// Update MCP configuration by disabling and re-enabling
			// Note: Tool names and metadata are auto-generated from route metadata
			await apiClient.bulkDisableMcp(currentTeam, { routeIds });
			await apiClient.bulkEnableMcp(currentTeam, { routeIds });

			// Reload MCP status to get updated state
			await loadMcpStatus();
		} catch (e) {
			mcpError = e instanceof Error ? e.message : 'Failed to save MCP configuration';
			console.error('Failed to save MCP configuration:', e);
		} finally {
			isSavingMcp = false;
		}
	}
</script>

{#if isLoading}
	<div class="min-h-screen bg-gray-100 flex items-center justify-center">
		<div class="flex flex-col items-center gap-3">
			<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
			<span class="text-sm text-gray-600">Loading configuration...</span>
		</div>
	</div>
{:else if error && !originalConfig}
	<div class="min-h-screen bg-gray-100 flex items-center justify-center">
		<div class="bg-white rounded-lg shadow-sm border border-red-200 p-8 max-w-md">
			<h2 class="text-xl font-bold text-red-900 mb-2">Error Loading Configuration</h2>
			<p class="text-sm text-red-700 mb-6">{error}</p>
			<Button onclick={handleCancel} variant="secondary">
				Back to List
			</Button>
		</div>
	</div>
{:else}
	<div class="min-h-screen bg-gray-50">
		<div class="px-8 py-8">
			<!-- Tabs -->
			<div class="mb-6">
				<div class="border-b border-gray-200">
					<nav class="-mb-px flex space-x-8" aria-label="Tabs">
						<button
							onclick={() => (activeTab = 'configuration')}
							class="{activeTab === 'configuration'
								? 'border-blue-500 text-blue-600'
								: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'} whitespace-nowrap py-4 px-1 border-b-2 font-medium text-sm"
						>
							Configuration
						</button>
						<button
							onclick={() => (activeTab = 'json')}
							class="{activeTab === 'json'
								? 'border-blue-500 text-blue-600'
								: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'} whitespace-nowrap py-4 px-1 border-b-2 font-medium text-sm"
						>
							JSON Preview
						</button>
					</nav>
				</div>
			</div>

			<!-- Tab Content -->
			{#if activeTab === 'configuration'}
				<!-- Header -->
				<div class="mb-8">
					<div class="flex items-center gap-4 mb-4">
						<button
							onclick={handleCancel}
							class="text-blue-600 hover:text-blue-800 transition-colors"
							title="Back to list"
						>
							<ArrowLeft class="w-6 h-6" />
						</button>
						<div>
							<h1 class="text-3xl font-bold text-gray-900">Edit Route Configuration</h1>
							<p class="text-sm text-gray-600 mt-1">
								Modify the route configuration for <span class="font-medium">{formState.name}</span>
							</p>
						</div>
					</div>
				</div>

				<!-- Error Message -->
				{#if error}
					<div class="mb-6 bg-red-50 border border-red-200 rounded-md p-4">
						<p class="text-sm text-red-800">{error}</p>
					</div>
				{/if}

				<!-- Basic Information -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Basic Information</h2>
					<div class="space-y-4">
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">
								Configuration Name <span class="text-red-500">*</span>
							</label>
							<input
								type="text"
								bind:value={formState.name}
								disabled
								class="w-full px-3 py-2 border border-gray-300 rounded-md bg-gray-100 text-gray-600 cursor-not-allowed"
							/>
							<p class="text-xs text-gray-500 mt-1">
								Configuration name cannot be changed after creation
							</p>
						</div>
					</div>
				</div>

				<!-- Virtual Hosts -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
					<div class="flex items-center justify-between mb-4">
						<div>
							<h2 class="text-lg font-semibold text-gray-900">Virtual Hosts</h2>
							<p class="text-sm text-gray-600">
								Each virtual host groups domains and their associated routes together
							</p>
						</div>
						<button
							onclick={handleAddVirtualHost}
							class="px-3 py-1.5 text-sm text-blue-600 border border-blue-600 rounded-md hover:bg-blue-50 transition-colors"
						>
							<Plus class="h-4 w-4 inline mr-1" />
							Add Virtual Host
						</button>
					</div>

					<div class="space-y-4">
						{#each formState.virtualHosts as vh, index}
							<VirtualHostEditor
								virtualHost={vh}
								{index}
								canRemove={formState.virtualHosts.length > 1}
								onUpdate={(updated) => handleUpdateVirtualHost(index, updated)}
								onRemove={() => handleRemoveVirtualHost(index)}
								{availableClusters}
								routeConfigName={configId || ''}
								configLevelFilters={attachedFilters}
								virtualHostFilters={virtualHostFiltersMap.get(vh.name) || []}
								routeFilters={routeFiltersMap.get(vh.name) || new Map()}
								onAddVirtualHostFilter={handleOpenVirtualHostFilterModal}
								onDetachVirtualHostFilter={handleRemoveVirtualHostFilter}
								onAddRouteFilter={handleOpenRouteFilterModal}
								onDetachRouteFilter={handleRemoveRouteFilter}
							/>
						{/each}
					</div>

					{#if formState.virtualHosts.length === 0}
						<div class="border-2 border-dashed border-gray-300 rounded-lg p-8 text-center">
							<p class="text-sm text-gray-600 mb-3">No virtual hosts defined</p>
							<button
								onclick={handleAddVirtualHost}
								class="px-4 py-2 text-sm text-blue-600 border border-blue-600 rounded-md hover:bg-blue-50 transition-colors"
							>
								<Plus class="h-4 w-4 inline mr-1" />
								Add Virtual Host
							</button>
						</div>
					{/if}

					<div class="mt-4 bg-blue-50 border border-blue-200 rounded-md p-3 text-sm text-blue-800">
						<strong>Note:</strong> Each virtual host groups domains and their routes together. Routes
						defined in a virtual host will only apply to the domains listed in that virtual host.
					</div>
				</div>

				<!-- Configured Filters (Config-level - Collapsible) -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 mb-6">
					<button
						onclick={() => (filtersExpanded = !filtersExpanded)}
						class="w-full px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
					>
						<div class="flex items-center gap-3">
							<Filter class="w-5 h-5 text-gray-500" />
							<div class="text-left">
								<h2 class="text-lg font-semibold text-gray-900">Configured Filters</h2>
								<p class="text-sm text-gray-600">
									Manage filters configured for all routes in this configuration
								</p>
							</div>
							{#if attachedFilters.length > 0}
								<span class="ml-2 px-2 py-0.5 text-xs font-medium rounded-full bg-blue-100 text-blue-800">
									{attachedFilters.length}
								</span>
							{/if}
						</div>
						{#if filtersExpanded}
							<ChevronUp class="w-5 h-5 text-gray-500" />
						{:else}
							<ChevronDown class="w-5 h-5 text-gray-500" />
						{/if}
					</button>
					{#if filtersExpanded}
						<div class="px-6 pb-6">
							{#if filterError}
								<div class="mb-4 bg-red-50 border border-red-200 rounded-md p-3">
									<p class="text-sm text-red-800">{filterError}</p>
								</div>
							{/if}

							<div class="flex items-center justify-between mb-4">
								<p class="text-sm text-gray-600">
									Filters are executed in order. Lower order numbers execute first.
								</p>
								<Button
									onclick={handleOpenConfigFilterModal}
									variant="secondary"
									disabled={isLoadingFilters}
								>
									<Plus class="h-4 w-4 mr-1" />
									Configure Filter
								</Button>
							</div>

							<FilterAttachmentList
								filters={attachedFilters}
								onDetach={handleRemoveConfigFilter}
								isLoading={isLoadingFilters}
								emptyMessage="No filters configured for this route configuration"
							/>

							<div class="mt-4 bg-amber-50 border border-amber-200 rounded-md p-3 text-sm text-amber-800">
								<strong>Note:</strong> Configured filters set per-route behavior for all routes in this configuration.
								For virtual-host-specific or route-specific filters, use the filter sections within each virtual host or route.
							</div>
						</div>
					{/if}
				</div>

				<!-- MCP Tool Configuration (Collapsible) -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 mb-6">
					<button
						onclick={() => (mcpExpanded = !mcpExpanded)}
						class="w-full px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
					>
						<div class="flex items-center gap-3">
							<Zap class="w-5 h-5 text-gray-500" />
							<div class="text-left">
								<h2 class="text-lg font-semibold text-gray-900">MCP Tool Configuration</h2>
								<p class="text-sm text-gray-600">
									Enable AI assistant integration for routes in this configuration
								</p>
							</div>
							{#if routeIds.length > 0 && mcpEnabledCount > 0}
								<span class="ml-2 px-2 py-0.5 text-xs font-medium rounded-full bg-emerald-100 text-emerald-800">
									{mcpEnabledCount} of {routeIds.length} enabled
								</span>
							{/if}
						</div>
						{#if mcpExpanded}
							<ChevronUp class="w-5 h-5 text-gray-500" />
						{:else}
							<ChevronDown class="w-5 h-5 text-gray-500" />
						{/if}
					</button>
					{#if mcpExpanded}
						<div class="px-6 pb-6 border-l-4 border-gray-200 ml-6">
							{#if isLoadingMcp}
								<div class="flex items-center gap-2 text-sm text-gray-600 py-4">
									<div class="animate-spin rounded-full h-4 w-4 border-b-2 border-blue-600"></div>
									Loading MCP status...
								</div>
							{:else}
								{#if mcpError}
									<div class="mb-4 bg-amber-50 border border-amber-200 rounded-md p-3">
										<p class="text-sm text-amber-800">{mcpError}</p>
									</div>
								{/if}

								<!-- Enable/Disable Toggle -->
								<div class="mb-6 flex items-center justify-between">
									<div>
										<label class="block text-sm font-medium text-gray-700 mb-1">
											MCP Tool Status
										</label>
										<p class="text-xs text-gray-500">
											{#if routeIds.length === 0}
												No routes found in this configuration
											{:else if mcpEnabled}
												{mcpEnabledCount} of {routeIds.length} routes exposed as MCP tools
											{:else}
												Enable to expose all {routeIds.length} routes to AI assistants
											{/if}
										</p>
									</div>
									<button
										onclick={handleToggleMcp}
										disabled={isSavingMcp || routeIds.length === 0}
										class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 {mcpEnabled
											? 'bg-emerald-600'
											: 'bg-gray-200'} {isSavingMcp || routeIds.length === 0 ? 'opacity-50 cursor-not-allowed' : ''}"
									>
										<span
											class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform {mcpEnabled
												? 'translate-x-6'
												: 'translate-x-1'}"
										></span>
									</button>
								</div>

								{#if mcpEnabled}
									<div class="space-y-4">
										<!-- Info about MCP Tools -->
										<div class="bg-blue-50 border border-blue-200 rounded-md p-4">
											<h3 class="text-sm font-medium text-blue-900 mb-2">MCP Tools Created</h3>
											<p class="text-sm text-blue-800">
												{mcpEnabledCount} route{mcpEnabledCount !== 1 ? 's are' : ' is'} now exposed as MCP tool{mcpEnabledCount !== 1 ? 's' : ''}.
												Tool names and schemas are automatically generated from route metadata and OpenAPI specifications.
											</p>
											{#if mcpToolName}
												<p class="text-xs text-blue-700 mt-2">
													Example tool name: <code class="bg-blue-100 px-1 py-0.5 rounded">{mcpToolName}</code>
												</p>
											{/if}
										</div>

										<!-- Schema Source Info -->
										{#if mcpStatus}
											<div>
												<label class="block text-sm font-medium text-gray-700 mb-2">
													Schema Information
												</label>
												<div class="space-y-2 text-sm text-gray-600">
													{#if mcpStatus.schemaSources?.openapi?.hasInputSchema}
														<div class="flex items-center gap-2">
															<span class="w-2 h-2 rounded-full bg-emerald-500"></span>
															<span>OpenAPI schema available</span>
														</div>
													{/if}
													{#if mcpStatus.schemaSources?.learned?.available}
														<div class="flex items-center gap-2">
															<span class="w-2 h-2 rounded-full bg-blue-500"></span>
															<span>Learned schema available ({mcpStatus.schemaSources.learned.sampleCount} samples)</span>
														</div>
													{/if}
													{#if mcpStatus.recommendedSource}
														<p class="text-xs text-gray-500 mt-2">
															Recommended source: <span class="font-medium">{mcpStatus.recommendedSource}</span>
														</p>
													{/if}
												</div>
											</div>
										{/if}
									</div>
								{/if}

								<div class="mt-4 bg-blue-50 border border-blue-200 rounded-md p-3 text-sm text-blue-800">
									<strong>Note:</strong> MCP (Model Context Protocol) allows AI assistants like Claude to
									call your APIs directly. The route must have proper metadata (operation ID, description)
									or learned schemas for best results.
								</div>
							{/if}
						</div>
					{/if}
				</div>

				<!-- Advanced (Collapsible) -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 mb-6">
					<button
						onclick={() => (advancedExpanded = !advancedExpanded)}
						class="w-full px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
					>
						<h2 class="text-lg font-semibold text-gray-900">Advanced Settings</h2>
						{#if advancedExpanded}
							<ChevronUp class="w-5 h-5 text-gray-500" />
						{:else}
							<ChevronDown class="w-5 h-5 text-gray-500" />
						{/if}
					</button>
					{#if advancedExpanded}
						<div class="px-6 pb-6">
							<p class="text-sm text-gray-600">Advanced settings will be added in future updates.</p>
						</div>
					{/if}
				</div>

				<!-- Action Buttons -->
				<div class="sticky bottom-0 bg-white border-t border-gray-200 p-4 -mx-8 flex justify-end gap-3">
					<Button onclick={handleCancel} variant="secondary" disabled={isSubmitting}>
						Cancel
					</Button>
					<Button onclick={handleSubmit} variant="primary" disabled={isSubmitting}>
						{isSubmitting ? 'Updating...' : 'Update Configuration'}
					</Button>
				</div>
			{:else}
				<!-- JSON Tab -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
					<JsonPanel jsonString={jsonPayload} editable={false} />
				</div>
			{/if}
		</div>
	</div>

	<!-- Filter Selector Modal -->
	<FilterSelectorModal
		isOpen={showFilterModal}
		filters={availableFilters}
		attachmentPoint="route"
		alreadyAttachedIds={attachedFilterIds()}
		onSelect={handleFilterModalSelect}
		onClose={() => {
			showFilterModal = false;
			currentFilterContext = null;
		}}
		isLoading={isLoadingFilters}
		hierarchyContext={currentFilterContext || undefined}
	/>
{/if}
