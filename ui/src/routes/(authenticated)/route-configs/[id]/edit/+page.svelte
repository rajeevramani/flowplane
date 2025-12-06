<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { Plus, ChevronDown, ChevronUp, ArrowLeft, Filter } from 'lucide-svelte';
	import type { ClusterResponse, RouteResponse, CreateRouteBody, FilterResponse, HierarchicalFilterContext } from '$lib/api/types';
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

			// Load filters (separate from main data load to not block UI)
			loadFilters();
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
	async function handleAttachConfigFilter(filterId: string, order?: number) {
		if (!configId) return;

		try {
			await apiClient.attachFilterToRouteConfig(configId, { filterId, order });
			await loadFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to attach filter';
			console.error('Failed to attach filter:', e);
		}
	}

	async function handleDetachConfigFilter(filterId: string) {
		if (!configId) return;

		try {
			await apiClient.detachFilterFromRouteConfig(configId, filterId);
			await loadFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to detach filter';
			console.error('Failed to detach filter:', e);
		}
	}

	// Virtual Host filter handlers
	function handleOpenVirtualHostFilterModal(virtualHostName: string) {
		currentFilterContext = {
			level: 'virtual_host',
			routeConfigName: configId!,
			virtualHostName
		};
		showFilterModal = true;
	}

	async function handleAttachVirtualHostFilter(virtualHostName: string, filterId: string, order?: number) {
		if (!configId) return;

		try {
			await apiClient.attachFilterToVirtualHost(configId, virtualHostName, { filterId, order });
			await loadHierarchicalFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to attach filter to virtual host';
			console.error('Failed to attach filter to virtual host:', e);
		}
	}

	async function handleDetachVirtualHostFilter(virtualHostName: string, filterId: string) {
		if (!configId) return;

		try {
			await apiClient.detachFilterFromVirtualHost(configId, virtualHostName, filterId);
			await loadHierarchicalFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to detach filter from virtual host';
			console.error('Failed to detach filter from virtual host:', e);
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
		showFilterModal = true;
	}

	async function handleAttachRouteFilter(virtualHostName: string, routeName: string, filterId: string, order?: number) {
		if (!configId) return;

		try {
			await apiClient.attachFilterToRoute(configId, virtualHostName, routeName, { filterId, order });
			await loadHierarchicalFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to attach filter to route';
			console.error('Failed to attach filter to route:', e);
		}
	}

	async function handleDetachRouteFilter(virtualHostName: string, routeName: string, filterId: string) {
		if (!configId) return;

		try {
			await apiClient.detachFilterFromRoute(configId, virtualHostName, routeName, filterId);
			await loadHierarchicalFilters();
		} catch (e) {
			filterError = e instanceof Error ? e.message : 'Failed to detach filter from route';
			console.error('Failed to detach filter from route:', e);
		}
	}

	// Universal filter modal handler
	async function handleFilterModalSelect(filterId: string, order?: number) {
		if (!currentFilterContext) return;

		switch (currentFilterContext.level) {
			case 'route_config':
				await handleAttachConfigFilter(filterId, order);
				break;
			case 'virtual_host':
				if (currentFilterContext.virtualHostName) {
					await handleAttachVirtualHostFilter(currentFilterContext.virtualHostName, filterId, order);
				}
				break;
			case 'route':
				if (currentFilterContext.virtualHostName && currentFilterContext.routeName) {
					await handleAttachRouteFilter(
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
		showFilterModal = true;
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
								virtualHostFilters={virtualHostFiltersMap.get(vh.name) || []}
								routeFilters={routeFiltersMap.get(vh.name) || new Map()}
								onAddVirtualHostFilter={handleOpenVirtualHostFilterModal}
								onDetachVirtualHostFilter={handleDetachVirtualHostFilter}
								onAddRouteFilter={handleOpenRouteFilterModal}
								onDetachRouteFilter={handleDetachRouteFilter}
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

				<!-- Attached Filters (Config-level - Collapsible) -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 mb-6">
					<button
						onclick={() => (filtersExpanded = !filtersExpanded)}
						class="w-full px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
					>
						<div class="flex items-center gap-3">
							<Filter class="w-5 h-5 text-gray-500" />
							<div class="text-left">
								<h2 class="text-lg font-semibold text-gray-900">Attached Filters</h2>
								<p class="text-sm text-gray-600">
									Manage filters applied to all routes in this configuration
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
									Attach Filter
								</Button>
							</div>

							<FilterAttachmentList
								filters={attachedFilters}
								onDetach={handleDetachConfigFilter}
								isLoading={isLoadingFilters}
								emptyMessage="No filters attached to this route configuration"
							/>

							<div class="mt-4 bg-amber-50 border border-amber-200 rounded-md p-3 text-sm text-amber-800">
								<strong>Note:</strong> Attached filters are applied to all routes in this configuration.
								For virtual-host-specific or route-specific filters, use the filter sections within each virtual host or route.
							</div>
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
