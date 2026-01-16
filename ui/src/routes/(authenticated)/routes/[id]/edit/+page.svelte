<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import {
		ArrowLeft,
		Save,
		Trash2,
		ChevronDown,
		ChevronUp,
		Plus,
		Settings,
		X,
		ExternalLink,
		Search
	} from 'lucide-svelte';
	import type {
		RouteRuleDefinition,
		PathMatchDefinition,
		RouteActionDefinition,
		ClusterResponse,
		FilterResponse,
		FilterTypeInfo,
		RouteHierarchyFiltersResponse,
		HierarchyFilterSummary
	} from '$lib/api/types';
	import type { SingleRouteEditData } from '$lib/api/routes';
	import {
		getSingleRouteForEdit,
		updateSingleRoute,
		deleteSingleRoute,
		getMcpStatusForRoute,
		enableMcpForRoute,
		disableMcpForRoute
	} from '$lib/api/routes';
	import { apiClient } from '$lib/api/client';
	import { RouteFilterCard } from '$lib/components/filters';
	import { selectedTeam } from '$lib/stores/team';

	// Route parameters
	const routeId = $derived($page.params.id);

	// Loading and error states
	let isLoading = $state(true);
	let isSaving = $state(false);
	let isDeleting = $state(false);
	let error = $state<string | null>(null);

	// Data
	let routeData = $state<SingleRouteEditData | null>(null);
	let clusters = $state<ClusterResponse[]>([]);
	let availableFilters = $state<FilterResponse[]>([]);
	let filterTypes = $state<Map<string, FilterTypeInfo>>(new Map());

	// Form state
	let routeName = $state('');
	let description = $state('');
	let domains = $state<string[]>([]);
	let pathMatchType = $state<'exact' | 'prefix' | 'regex' | 'template'>('prefix');
	let pathValue = $state('');
	let httpMethods = $state<Set<string>>(new Set(['GET', 'POST']));
	let primaryCluster = $state('');
	let timeoutSeconds = $state<number | undefined>(30);
	let maxRetries = $state<number | undefined>(3);
	let prefixRewrite = $state('');

	// Collapsible sections
	let pathRewriteExpanded = $state(false);
	let mcpExpanded = $state(false);
	let filtersExpanded = $state(false);

	// MCP state
	let mcpEnabled = $state(false);
	let mcpToolName = $state('');
	let mcpDescription = $state('');
	let mcpSchemaSource = $state<'openapi' | 'learned' | 'manual'>('openapi');

	// Filter state
	let routeFilters = $state<FilterResponse[]>([]);
	let virtualHostFilters = $state<FilterResponse[]>([]);
	let routeConfigFilters = $state<FilterResponse[]>([]);
	let loadingFilters = $state(false);
	let showFilterDropdown = $state(false);
	let filterSearchQuery = $state('');

	// Helper to format dates - simple relative time formatter
	function formatDate(dateString: string): string {
		const date = new Date(dateString);
		const now = new Date();
		const diffMs = now.getTime() - date.getTime();
		const diffMins = Math.floor(diffMs / 60000);
		const diffHours = Math.floor(diffMins / 60);
		const diffDays = Math.floor(diffHours / 24);

		if (diffMins < 1) return 'just now';
		if (diffMins < 60) return `${diffMins}m ago`;
		if (diffHours < 24) return `${diffHours}h ago`;
		if (diffDays < 7) return `${diffDays}d ago`;
		return date.toLocaleDateString();
	}

	// Load all data
	onMount(async () => {
		await loadData();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		try {
			const [route, clustersData, filtersData] = await Promise.all([
				getSingleRouteForEdit(routeId),
				apiClient.listClusters(),
				apiClient.listFilters()
			]);

			routeData = route;
			clusters = clustersData;
			availableFilters = filtersData;

			// Populate form from route data
			routeName = route.route.name || '';
			description = route.view.summary || '';
			domains = [...route.virtualHost.domains];

			// Extract path match
			const pathMatch = route.route.match.path;
			pathMatchType = pathMatch.type;
			pathValue = pathMatch.type === 'template' ? pathMatch.template || '' : pathMatch.value || '';

			// Extract HTTP methods from match.headers
			const methodsSet = new Set<string>();
			if (route.route.match.headers) {
				const methodHeader = route.route.match.headers.find((h: { name: string }) => h.name === ':method');
				if (methodHeader && 'value' in methodHeader && methodHeader.value) {
					methodHeader.value.split('|').forEach((m: string) => methodsSet.add(m.trim()));
				}
			}
			if (methodsSet.size === 0) {
				methodsSet.add('GET');
			}
			httpMethods = methodsSet;

			// Extract action details
			if (route.route.action.type === 'forward') {
				primaryCluster = route.route.action.cluster;
				timeoutSeconds = route.route.action.timeoutSeconds;
				prefixRewrite = route.route.action.prefixRewrite || '';
				maxRetries = route.route.action.retryPolicy?.maxRetries;
			}

			// Load MCP status if route ID exists
			if (route.view.mcpEnabled && route.view.routeId && $selectedTeam) {
				try {
					const mcpStatus = await getMcpStatusForRoute($selectedTeam, route.view.routeId);
					mcpEnabled = mcpStatus.enabled;
					mcpToolName = mcpStatus.toolName || '';
					mcpDescription = mcpStatus.metadata?.description || '';
				} catch (err) {
					console.error('Failed to load MCP status:', err);
				}
			}

			// Load filters for this route
			await loadRouteFilters(route);
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load route data';
			console.error('Load error:', err);
		} finally {
			isLoading = false;
		}
	}

	// Build updated route object
	function buildUpdatedRoute(): RouteRuleDefinition {
		const pathMatch: PathMatchDefinition = {
			type: pathMatchType,
			...(pathMatchType === 'template' ? { template: pathValue } : { value: pathValue })
		};

		const action: RouteActionDefinition = {
			type: 'forward',
			cluster: primaryCluster,
			timeoutSeconds: timeoutSeconds,
			prefixRewrite: prefixRewrite || undefined,
			retryPolicy: maxRetries ? { maxRetries, retryOn: ['5xx', 'reset'] } : undefined
		};

		// Build headers array - only add method if methods are selected
		const headers: Array<{ name: string; value: string }> = [];
		if (httpMethods.size > 0) {
			headers.push({
				name: ':method',
				value: Array.from(httpMethods).join('|')
			});
		}

		return {
			name: routeName,
			match: {
				path: pathMatch,
				headers: headers.length > 0 ? headers : undefined
			},
			action,
			typedPerFilterConfig: routeData?.route.typedPerFilterConfig
		};
	}

	// Save changes
	async function handleSave() {
		if (!routeData) return;

		isSaving = true;
		error = null;

		try {
			const updatedRoute = buildUpdatedRoute();
			await updateSingleRoute(
				routeData.config.name,
				routeData.virtualHostIndex,
				routeData.routeIndex,
				updatedRoute
			);

			// Navigate back to the list
			await goto('/route-configs');
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to save route';
			console.error('Save error:', err);
		} finally {
			isSaving = false;
		}
	}

	// Delete route
	async function handleDelete() {
		if (!routeData) return;
		if (!confirm('Are you sure you want to delete this route? This action cannot be undone.')) {
			return;
		}

		isDeleting = true;
		error = null;

		try {
			await deleteSingleRoute(
				routeData.config.name,
				routeData.virtualHostIndex,
				routeData.routeIndex
			);

			// Navigate back to the list
			await goto('/route-configs');
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete route';
			console.error('Delete error:', err);
		} finally {
			isDeleting = false;
		}
	}

	// Cancel and go back
	function handleCancel() {
		goto('/route-configs');
	}

	// Domain management
	function addDomain() {
		domains = [...domains, ''];
	}

	function updateDomain(index: number, value: string) {
		const newDomains = [...domains];
		newDomains[index] = value;
		domains = newDomains;
	}

	function removeDomain(index: number) {
		domains = domains.filter((_, i) => i !== index);
	}

	// HTTP method toggles
	function toggleMethod(method: string) {
		const newMethods = new Set(httpMethods);
		if (newMethods.has(method)) {
			newMethods.delete(method);
		} else {
			newMethods.add(method);
		}
		httpMethods = newMethods;
	}

	// MCP toggle
	async function toggleMcp() {
		if (!routeData || !$selectedTeam) return;

		try {
			if (mcpEnabled) {
				await disableMcpForRoute($selectedTeam, routeData.view.routeId);
				mcpEnabled = false;
			} else {
				await enableMcpForRoute($selectedTeam, routeData.view.routeId, {
					toolName: mcpToolName || undefined,
					description: mcpDescription || undefined,
					schemaSource: mcpSchemaSource
				});
				mcpEnabled = true;
			}
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to toggle MCP';
			console.error('MCP toggle error:', err);
		}
	}

	// Load filters for this route
	async function loadRouteFilters(route: SingleRouteEditData) {
		loadingFilters = true;
		try {
			// Load hierarchy filters for this route
			const hierarchyFilters = await apiClient.listRouteHierarchyFilters(
				route.config.name,
				route.virtualHost.name,
				route.route.name
			);

			// The hierarchy filters endpoint returns route-level filters
			routeFilters = hierarchyFilters.filters || [];
			console.debug('Loaded route filters:', routeFilters);

			// Load virtual host filters
			try {
				const vhFilters = await apiClient.listVirtualHostFilters(
					route.config.name,
					route.virtualHost.name
				);
				virtualHostFilters = vhFilters.filters || [];
			} catch {
				virtualHostFilters = [];
			}

			// Load route config filters
			try {
				const configFilters = await apiClient.listRouteConfigFilters(route.config.name);
				configFilters.filters || [];
				routeConfigFilters = configFilters.filters || [];
			} catch {
				routeConfigFilters = [];
			}
		} catch (err) {
			console.error('Failed to load route filters:', err);
		} finally {
			loadingFilters = false;
		}
	}

	// Filter management
	async function attachFilter(filter: FilterResponse) {
		if (!routeData) return;

		try {
			// Use configureFilter with route scope
			await apiClient.configureFilter(filter.id, {
				scopeType: 'route',
				scopeId: `${routeData.config.name}/${routeData.virtualHost.name}/${routeData.route.name}`
			});
			// Reload filters
			await loadRouteFilters(routeData);
			showFilterDropdown = false;
			filterSearchQuery = '';
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to attach filter';
		}
	}

	async function detachFilter(filterId: string) {
		if (!routeData) return;

		try {
			// Use removeFilterConfiguration with route scope
			await apiClient.removeFilterConfiguration(
				filterId,
				'route',
				`${routeData.config.name}/${routeData.virtualHost.name}/${routeData.route.name}`
			);
			// Reload filters
			await loadRouteFilters(routeData);
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to detach filter';
		}
	}

	// Get filters available to attach (not already attached)
	const attachableFilters = $derived(() => {
		const attachedNames = new Set(routeFilters.map((f) => f.name));
		return availableFilters.filter((f) => !attachedNames.has(f.name));
	});

	// Filter search results
	const filteredAvailableFilters = $derived(() => {
		const filters = attachableFilters();
		if (!filterSearchQuery.trim()) return filters;
		const query = filterSearchQuery.toLowerCase();
		return filters.filter(
			(f) =>
				f.name.toLowerCase().includes(query) ||
				f.filterType.toLowerCase().includes(query)
		);
	});
</script>

<!-- Loading State -->
{#if isLoading}
	<div class="min-h-screen flex items-center justify-center">
		<div class="text-center">
			<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600 mx-auto"></div>
			<p class="mt-4 text-gray-600">Loading route...</p>
		</div>
	</div>
{:else if !routeData}
	<div class="min-h-screen flex items-center justify-center">
		<div class="text-center">
			<p class="text-red-600 text-lg">Route not found</p>
			<button
				onclick={handleCancel}
				class="mt-4 px-4 py-2 bg-gray-100 text-gray-700 rounded-md hover:bg-gray-200"
			>
				Go Back
			</button>
		</div>
	</div>
{:else}
	<!-- Top Navigation Bar -->
	<div class="bg-white border-b border-gray-200 sticky top-0 z-10">
		<div class="px-6 py-4">
			<div class="flex items-center justify-between">
				<div class="flex items-center gap-4">
					<button
						onclick={handleCancel}
						class="p-2 text-gray-500 hover:text-gray-700 hover:bg-gray-100 rounded-md transition-colors"
					>
						<ArrowLeft class="h-5 w-5" />
					</button>
					<div>
						<div class="flex items-center gap-2">
							<h1 class="text-xl font-bold text-gray-900">Edit Route</h1>
							<span class="px-2 py-0.5 text-xs font-medium bg-gray-100 text-gray-600 rounded">
								{routeData.config.name}
							</span>
						</div>
						<p class="text-sm text-gray-500">API Gateway route configuration</p>
					</div>
				</div>
				<div class="flex items-center gap-3">
					<button
						onclick={handleCancel}
						class="px-4 py-2 text-gray-700 border border-gray-300 rounded-md hover:bg-gray-50 transition-colors"
						disabled={isSaving || isDeleting}
					>
						Cancel
					</button>
					<button
						onclick={handleSave}
						class="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
						disabled={isSaving || isDeleting}
					>
						{isSaving ? 'Saving...' : 'Save Changes'}
					</button>
				</div>
			</div>
		</div>
	</div>

	<!-- Main Content -->
	<div class="px-6 py-8 space-y-6">
		<!-- Error Message -->
		{#if error}
			<div class="bg-red-50 border border-red-200 rounded-lg p-4">
				<p class="text-red-800 text-sm">{error}</p>
			</div>
		{/if}

		<!-- Route Context Banner -->
		<div class="bg-blue-50 border border-blue-200 rounded-lg p-4">
			<div class="flex items-center justify-between">
				<div class="flex items-center gap-6 text-sm">
					<div>
						<span class="text-blue-600">Configuration:</span>
						<a
							href="/route-configs/{routeData.config.name}/edit"
							class="ml-1 text-blue-800 font-medium hover:underline"
						>
							{routeData.config.name}
						</a>
					</div>
					<div>
						<span class="text-blue-600">Virtual Host:</span>
						<span class="ml-1 text-blue-800 font-medium">{routeData.virtualHost.name}</span>
					</div>
				</div>
				<div class="flex items-center gap-4 text-xs text-blue-600">
					<span>Created: {formatDate(routeData.view.createdAt)}</span>
					<span>Modified: {formatDate(routeData.view.updatedAt)}</span>
				</div>
			</div>
		</div>

		<!-- Basic Information -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Basic Information</h2>
			<div class="space-y-4">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Route Name <span class="text-red-500">*</span>
					</label>
					<input
						type="text"
						bind:value={routeName}
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						placeholder="Enter route name"
					/>
					<p class="text-xs text-gray-500 mt-1">A descriptive name for this route</p>
				</div>
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Description</label>
					<textarea
						rows="2"
						bind:value={description}
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						placeholder="Optional description..."
					></textarea>
				</div>
			</div>
		</div>

		<!-- Request Matching -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Request Matching</h2>
			<p class="text-sm text-gray-500 mb-4">Define how incoming requests match this route</p>

			<div class="space-y-6">
				<!-- Domains -->
				<div>
					<div class="flex items-center justify-between mb-2">
						<label class="block text-sm font-medium text-gray-700">Domains</label>
						<button
							onclick={addDomain}
							class="text-sm text-blue-600 hover:text-blue-800 transition-colors"
						>
							+ Add Domain
						</button>
					</div>
					<div class="space-y-2">
						{#each domains as domain, index}
							<div class="flex gap-2 items-center">
								<input
									type="text"
									value={domain}
									oninput={(e) => updateDomain(index, e.currentTarget.value)}
									class="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									placeholder="example.com"
								/>
								<button
									onclick={() => removeDomain(index)}
									class="p-2 text-gray-400 hover:text-red-600 transition-colors"
								>
									<Trash2 class="h-5 w-5" />
								</button>
							</div>
						{/each}
					</div>
				</div>

				<!-- Divider -->
				<div class="border-t border-gray-200"></div>

				<!-- Path Matching -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-2">Path</label>
					<div class="grid grid-cols-4 gap-4">
						<div>
							<label class="block text-xs text-gray-500 mb-1">Match Type</label>
							<select
								bind:value={pathMatchType}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							>
								<option value="prefix">Prefix</option>
								<option value="exact">Exact</option>
								<option value="template">Template</option>
								<option value="regex">Regex</option>
							</select>
						</div>
						<div class="col-span-3">
							<label class="block text-xs text-gray-500 mb-1">
								Path Pattern <span class="text-red-500">*</span>
							</label>
							<input
								type="text"
								bind:value={pathValue}
								class="w-full px-3 py-2 font-mono text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								placeholder="/api/v1/*"
							/>
						</div>
					</div>
				</div>

				<!-- HTTP Methods -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-2">HTTP Methods</label>
					<div class="flex flex-wrap gap-2">
						{#each ['GET', 'POST', 'PUT', 'DELETE', 'PATCH'] as method}
							<label
								class="inline-flex items-center px-3 py-2 border border-gray-300 rounded-md cursor-pointer hover:bg-gray-50 transition-colors"
							>
								<input
									type="checkbox"
									checked={httpMethods.has(method)}
									onchange={() => toggleMethod(method)}
									class="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
								/>
								<span class="ml-2 text-sm text-gray-700">{method}</span>
							</label>
						{/each}
					</div>
				</div>

				<!-- Divider -->
				<div class="border-t border-gray-200"></div>

				<!-- Path Rewrite (collapsible) -->
				<div>
					<button
						onclick={() => (pathRewriteExpanded = !pathRewriteExpanded)}
						class="w-full flex items-center justify-between text-left"
					>
						<div class="flex items-center gap-2">
							<span class="text-sm font-medium text-gray-700">Path Rewrite</span>
							<span class="text-xs text-gray-400">(optional)</span>
						</div>
						{#if pathRewriteExpanded}
							<ChevronUp class="h-4 w-4 text-gray-500" />
						{:else}
							<ChevronDown class="h-4 w-4 text-gray-500" />
						{/if}
					</button>
					{#if pathRewriteExpanded}
						<div class="mt-4 space-y-4 pl-4 border-l-2 border-gray-100">
							<div>
								<label class="block text-sm font-medium text-gray-700 mb-1">Prefix Rewrite</label>
								<input
									type="text"
									bind:value={prefixRewrite}
									placeholder="/api/v1 -> /"
									class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<p class="text-xs text-gray-500 mt-1">Replace the matched prefix with this value</p>
							</div>
						</div>
					{/if}
				</div>
			</div>
		</div>

		<!-- Upstream Cluster -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Upstream Cluster</h2>
			<div class="space-y-4">
				<div class="grid grid-cols-2 gap-4">
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Primary Cluster <span class="text-red-500">*</span>
						</label>
						<select
							bind:value={primaryCluster}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						>
							<option value="">Select cluster</option>
							{#each clusters as cluster}
								<option value={cluster.name}>{cluster.name}</option>
							{/each}
						</select>
					</div>
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Timeout (seconds)</label>
						<input
							type="number"
							bind:value={timeoutSeconds}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							min="0"
						/>
					</div>
				</div>
				<div class="grid grid-cols-2 gap-4">
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Max Retries</label>
						<input
							type="number"
							bind:value={maxRetries}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							min="0"
						/>
					</div>
				</div>
			</div>
		</div>

		<!-- Request Processing (MCP Tool + Filters) -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Request Processing</h2>
			<p class="text-sm text-gray-500 mb-4">Configure middleware and tools for this route</p>

			<div class="space-y-6">
				<!-- MCP Tool -->
				<div>
					<button
						onclick={() => (mcpExpanded = !mcpExpanded)}
						class="w-full flex items-center justify-between text-left"
					>
						<div class="flex items-center gap-2">
							<span class="text-sm font-medium text-gray-700">MCP Tool</span>
							{#if mcpEnabled}
								<span class="px-2 py-0.5 text-xs font-medium bg-emerald-100 text-emerald-700 rounded-full">
									Enabled
								</span>
							{/if}
						</div>
						{#if mcpExpanded}
							<ChevronUp class="h-4 w-4 text-gray-500" />
						{:else}
							<ChevronDown class="h-4 w-4 text-gray-500" />
						{/if}
					</button>
					{#if mcpExpanded}
						<div class="mt-4 pl-4 border-l-2 border-gray-100 space-y-4">
							<div class="flex items-center justify-between p-3 bg-gray-50 rounded-lg">
								<span class="text-sm font-medium text-gray-700">Create Tool</span>
								<button
									onclick={toggleMcp}
									class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors"
									class:bg-emerald-500={mcpEnabled}
									class:bg-gray-300={!mcpEnabled}
								>
									<span
										class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
										class:translate-x-6={mcpEnabled}
										class:translate-x-1={!mcpEnabled}
									></span>
								</button>
							</div>
							{#if mcpEnabled}
								<div class="grid grid-cols-2 gap-4">
									<div>
										<label class="block text-sm font-medium text-gray-700 mb-1">Tool Name</label>
										<input
											type="text"
											bind:value={mcpToolName}
											class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
											placeholder="get_api_data"
										/>
									</div>
									<div>
										<label class="block text-sm font-medium text-gray-700 mb-1">Schema Source</label>
										<select
											bind:value={mcpSchemaSource}
											class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
										>
											<option value="openapi">Auto (Learned)</option>
											<option value="learned">OpenAPI Spec</option>
											<option value="manual">Manual</option>
										</select>
									</div>
									<div class="col-span-2">
										<label class="block text-sm font-medium text-gray-700 mb-1">Description</label>
										<textarea
											rows="2"
											bind:value={mcpDescription}
											class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
											placeholder="Describe what this tool does..."
										></textarea>
									</div>
								</div>
							{/if}
						</div>
					{/if}
				</div>

				<!-- Divider -->
				<div class="border-t border-gray-200"></div>

				<!-- Filters -->
				<div>
					<button
						onclick={() => (filtersExpanded = !filtersExpanded)}
						class="w-full flex items-center justify-between text-left"
					>
						<div class="flex items-center gap-2">
							<span class="text-sm font-medium text-gray-700">Filters</span>
							<span class="px-2 py-0.5 text-xs font-medium bg-gray-100 text-gray-600 rounded-full">
								{routeFilters.length} attached
							</span>
						</div>
						{#if filtersExpanded}
							<ChevronUp class="h-4 w-4 text-gray-500" />
						{:else}
							<ChevronDown class="h-4 w-4 text-gray-500" />
						{/if}
					</button>
					{#if filtersExpanded}
						<div class="mt-4 pl-4 border-l-2 border-gray-100">
							<div class="flex items-center justify-between mb-4">
								<p class="text-sm text-gray-500">Manage filters attached to this route</p>
								<div class="relative">
									<button
										onclick={(e) => {
											e.stopPropagation();
											showFilterDropdown = !showFilterDropdown;
										}}
										class="text-sm text-blue-600 hover:text-blue-800 flex items-center gap-1"
									>
										+ Attach Filter
										<ChevronDown class="h-4 w-4" />
									</button>
									<!-- Dropdown -->
									{#if showFilterDropdown}
										<div class="absolute right-0 mt-2 w-64 bg-white rounded-lg shadow-lg border border-gray-200 z-20">
											<div class="p-2 border-b border-gray-200">
												<div class="relative">
													<Search class="absolute left-2 top-1/2 transform -translate-y-1/2 h-4 w-4 text-gray-400" />
													<input
														type="text"
														placeholder="Search filters..."
														bind:value={filterSearchQuery}
														class="w-full pl-8 pr-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
													/>
												</div>
											</div>
											<div class="max-h-48 overflow-y-auto py-1">
												{#each filteredAvailableFilters() as filter}
													<button
														onclick={() => attachFilter(filter)}
														class="w-full px-3 py-2 text-left text-sm hover:bg-gray-50 flex items-center justify-between"
													>
														<span>{filter.name}</span>
														<span class="text-xs text-gray-400">{filter.filterType}</span>
													</button>
												{:else}
													<div class="px-3 py-2 text-sm text-gray-500">No filters available</div>
												{/each}
											</div>
											<div class="p-2 border-t border-gray-200">
												<a
													href="/filters"
													class="flex items-center gap-2 text-sm text-blue-600 hover:text-blue-800"
												>
													<ExternalLink class="h-4 w-4" />
													Manage Filters
												</a>
											</div>
										</div>
									{/if}
								</div>
							</div>

							<!-- Loading state -->
							{#if loadingFilters}
								<div class="text-sm text-gray-500">Loading filters...</div>
							{:else}
								<!-- Direct Filters (attached at route level) -->
								<div class="space-y-3">
									{#each routeFilters as filter}
										<RouteFilterCard
											{filter}
											routeConfigName={routeData.config.name}
											virtualHostName={routeData.virtualHost.name}
											routeName={routeData.route.name || ''}
											isInherited={false}
											onRemove={() => detachFilter(filter.id)}
											onSettingsUpdate={() => routeData && loadRouteFilters(routeData)}
										/>
									{:else}
										<div class="text-sm text-gray-500 py-2">No filters attached to this route</div>
									{/each}
								</div>

								<!-- Inherited Filters from Virtual Host -->
								{#if virtualHostFilters.length > 0}
									<div class="mt-4 pt-4 border-t border-gray-200">
										<p class="text-sm text-gray-500 mb-3">Inherited from Virtual Host</p>
										<div class="space-y-3">
											{#each virtualHostFilters as filter}
												<RouteFilterCard
													{filter}
													routeConfigName={routeData.config.name}
													virtualHostName={routeData.virtualHost.name}
													routeName={routeData.route.name || ''}
													isInherited={true}
													onSettingsUpdate={() => routeData && loadRouteFilters(routeData)}
												/>
											{/each}
										</div>
									</div>
								{/if}

								<!-- Inherited Filters from Route Config -->
								{#if routeConfigFilters.length > 0}
									<div class="mt-4 pt-4 border-t border-gray-200">
										<p class="text-sm text-gray-500 mb-3">Inherited from Route Config</p>
										<div class="space-y-3">
											{#each routeConfigFilters as filter}
												<RouteFilterCard
													{filter}
													routeConfigName={routeData.config.name}
													virtualHostName={routeData.virtualHost.name}
													routeName={routeData.route.name || ''}
													isInherited={true}
													onSettingsUpdate={() => routeData && loadRouteFilters(routeData)}
												/>
											{/each}
										</div>
									</div>
								{/if}
							{/if}
						</div>
					{/if}
				</div>
			</div>
		</div>

		<!-- Danger Zone -->
		<div class="bg-white rounded-lg shadow-sm border border-red-200 p-6">
			<h2 class="text-lg font-semibold text-red-900 mb-2">Danger Zone</h2>
			<p class="text-sm text-gray-600 mb-4">
				Permanently delete this route. This action cannot be undone.
			</p>
			<button
				onclick={handleDelete}
				class="px-4 py-2 text-red-600 border border-red-300 rounded-md hover:bg-red-50 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
				disabled={isDeleting || isSaving}
			>
				{isDeleting ? 'Deleting...' : 'Delete Route'}
			</button>
		</div>
	</div>
{/if}
