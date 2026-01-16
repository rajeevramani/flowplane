<script lang="ts">
	import { Plus, Trash2, ChevronDown, ChevronUp, Filter, X } from 'lucide-svelte';
	import type { VirtualHostDefinition, RouteRuleDefinition, FilterResponse, HierarchyLevel } from '$lib/api/types';
	import { RouteFilterSection } from '$lib/components/route-config';

	interface Props {
		virtualHost: VirtualHostFormState;
		index: number;
		canRemove: boolean;
		onUpdate: (vh: VirtualHostFormState) => void;
		onRemove: () => void;
		availableClusters: string[];
		// Hierarchical filter props
		routeConfigName: string;
		configLevelFilters?: FilterResponse[];
		virtualHostFilters?: FilterResponse[];
		routeFilters?: Map<string, FilterResponse[]>; // routeName -> filters
		onAddVirtualHostFilter?: (virtualHostName: string) => void;
		onDetachVirtualHostFilter?: (virtualHostName: string, filterId: string) => void;
		onAddRouteFilter?: (virtualHostName: string, routeName: string) => void;
		onDetachRouteFilter?: (virtualHostName: string, routeName: string, filterId: string) => void;
	}

	export interface VirtualHostFormState {
		id: string;
		name: string;
		domains: string[];
		routes: RouteFormState[];
	}

	export interface RouteFormState {
		id: string;
		name: string;
		method: string;
		path: string;
		pathType: 'prefix' | 'exact' | 'template' | 'regex';
		cluster: string;
		timeout?: number;
		// Path rewrites
		prefixRewrite?: string;
		templateRewrite?: string;
		// Retry policy
		retryEnabled?: boolean;
		maxRetries?: number;
		retryOn?: string[];
		perTryTimeout?: number;
		backoffBaseMs?: number;
		backoffMaxMs?: number;
	}

	let {
		virtualHost,
		index,
		canRemove,
		onUpdate,
		onRemove,
		availableClusters,
		routeConfigName,
		configLevelFilters = [],
		virtualHostFilters = [],
		routeFilters = new Map(),
		onAddVirtualHostFilter,
		onDetachVirtualHostFilter,
		onAddRouteFilter,
		onDetachRouteFilter
	}: Props = $props();

	let isExpanded = $state(true);
	let newDomain = $state('');
	let expandedRoutes = $state<Set<string>>(new Set());
	let routeFiltersExpanded = $state<Set<string>>(new Set());

	// Placeholder text with curly braces for template examples
	const templatePlaceholder = '/users/{user_id}';

	// Get filter type display label
	function getFilterTypeLabel(filterType: string): string {
		switch (filterType) {
			case 'header_mutation':
				return 'Header Mutation';
			case 'jwt_auth':
			case 'jwt_authn':
				return 'JWT Auth';
			case 'cors':
				return 'CORS';
			case 'rate_limit':
			case 'local_rate_limit':
				return 'Rate Limit';
			case 'ext_authz':
				return 'External Auth';
			default:
				return filterType;
		}
	}

	// Get filter type badge color
	function getFilterTypeBadgeColor(filterType: string): string {
		switch (filterType) {
			case 'header_mutation':
				return 'bg-blue-100 text-blue-800';
			case 'jwt_auth':
			case 'jwt_authn':
				return 'bg-green-100 text-green-800';
			case 'cors':
				return 'bg-purple-100 text-purple-800';
			case 'rate_limit':
			case 'local_rate_limit':
				return 'bg-orange-100 text-orange-800';
			case 'ext_authz':
				return 'bg-red-100 text-red-800';
			default:
				return 'bg-gray-100 text-gray-800';
		}
	}

	// Check if route has filters attached
	function getRouteFilterCount(routeName: string): number {
		return routeFilters.get(routeName)?.length ?? 0;
	}

	// Add domain
	function handleAddDomain() {
		if (newDomain.trim()) {
			const updated = {
				...virtualHost,
				domains: [...virtualHost.domains, newDomain.trim()]
			};
			onUpdate(updated);
			newDomain = '';
		}
	}

	// Remove domain
	function handleRemoveDomain(domainIndex: number) {
		const updated = {
			...virtualHost,
			domains: virtualHost.domains.filter((_, i) => i !== domainIndex)
		};
		onUpdate(updated);
	}

	// Add route
	function handleAddRoute() {
		const newRoute: RouteFormState = {
			id: `route-${Date.now()}`,
			name: `route-${virtualHost.routes.length + 1}`,
			method: 'GET',
			path: '/',
			pathType: 'prefix',
			cluster: availableClusters[0] || '',
			timeout: 30
		};
		const updated = {
			...virtualHost,
			routes: [...virtualHost.routes, newRoute]
		};
		onUpdate(updated);
	}

	// Remove route
	function handleRemoveRoute(routeId: string) {
		const updated = {
			...virtualHost,
			routes: virtualHost.routes.filter((r) => r.id !== routeId)
		};
		onUpdate(updated);
	}

	// Update route
	function handleUpdateRoute(routeId: string, field: keyof RouteFormState, value: any) {
		const updated = {
			...virtualHost,
			routes: virtualHost.routes.map((r) => {
				if (r.id !== routeId) return r;

				const updatedRoute = { ...r, [field]: value };

				// Clear incompatible rewrite fields when pathType changes
				if (field === 'pathType') {
					if (value === 'template') {
						// Template match type: clear prefixRewrite, keep templateRewrite
						updatedRoute.prefixRewrite = undefined;
					} else {
						// Non-template match types: clear templateRewrite, keep prefixRewrite
						updatedRoute.templateRewrite = undefined;
					}
				}

				return updatedRoute;
			})
		};
		onUpdate(updated);
	}

	// Toggle expand/collapse
	function toggleExpand() {
		isExpanded = !isExpanded;
	}

	// Toggle route advanced settings
	function toggleRouteAdvanced(routeId: string) {
		if (expandedRoutes.has(routeId)) {
			expandedRoutes.delete(routeId);
		} else {
			expandedRoutes.add(routeId);
		}
		expandedRoutes = new Set(expandedRoutes); // Trigger reactivity
	}

	// Toggle route filters expansion
	function toggleRouteFiltersExpanded(routeId: string) {
		if (routeFiltersExpanded.has(routeId)) {
			routeFiltersExpanded.delete(routeId);
		} else {
			routeFiltersExpanded.add(routeId);
		}
		routeFiltersExpanded = new Set(routeFiltersExpanded);
	}

	// Get inherited filters for a route (combines config-level + VH-level)
	function getInheritedFiltersForRoute(): FilterResponse[] {
		const configFilters = configLevelFilters || [];
		const vhFilters = virtualHostFilters || [];
		return [...configFilters, ...vhFilters];
	}

	// Retry policy presets
	function applyRetryPreset(routeId: string, preset: 'conservative' | 'balanced' | 'aggressive') {
		const presets = {
			conservative: {
				maxRetries: 2,
				perTryTimeout: 5,
				retryOn: ['5xx', 'reset', 'connect-failure'],
				backoffBaseMs: 100,
				backoffMaxMs: 500
			},
			balanced: {
				maxRetries: 3,
				perTryTimeout: 10,
				retryOn: ['5xx', 'reset', 'connect-failure', 'retriable-4xx'],
				backoffBaseMs: 100,
				backoffMaxMs: 1000
			},
			aggressive: {
				maxRetries: 5,
				perTryTimeout: 15,
				retryOn: ['5xx', 'reset', 'connect-failure', 'retriable-4xx', 'gateway-error'],
				backoffBaseMs: 200,
				backoffMaxMs: 2000
			}
		};

		const config = presets[preset];
		const updated = {
			...virtualHost,
			routes: virtualHost.routes.map((r) =>
				r.id === routeId
					? {
							...r,
							retryEnabled: true,
							maxRetries: config.maxRetries,
							perTryTimeout: config.perTryTimeout,
							retryOn: config.retryOn,
							backoffBaseMs: config.backoffBaseMs,
							backoffMaxMs: config.backoffMaxMs
					  }
					: r
			)
		};
		onUpdate(updated);
	}
</script>

<div class="border border-gray-200 rounded-lg mb-4 bg-white">
	<!-- Header -->
	<div
		class="flex items-center justify-between p-4 bg-gray-50 rounded-t-lg cursor-pointer hover:bg-gray-100 transition-colors"
		onclick={toggleExpand}
		role="button"
		tabindex="0"
	>
		<div class="flex items-center gap-3">
			<button
				onclick={(e) => {
					e.stopPropagation();
					toggleExpand();
				}}
				class="text-gray-500 hover:text-gray-700"
			>
				{#if isExpanded}
					<ChevronUp class="h-5 w-5" />
				{:else}
					<ChevronDown class="h-5 w-5" />
				{/if}
			</button>
			<h3 class="font-medium text-gray-900">
				Virtual Host #{index + 1}
				{#if virtualHost.domains.length > 0}
					<span class="text-sm text-gray-500 font-normal ml-2">
						({virtualHost.domains[0]}{virtualHost.domains.length > 1
							? ` +${virtualHost.domains.length - 1} more`
							: ''})
					</span>
				{/if}
			</h3>
			<span class="text-xs text-gray-500">
				{virtualHost.routes.length} route{virtualHost.routes.length !== 1 ? 's' : ''}
			</span>
			{#if virtualHostFilters.length > 0}
				<span class="px-2 py-0.5 text-xs font-medium rounded-full bg-emerald-100 text-emerald-800">
					{virtualHostFilters.length} filter{virtualHostFilters.length !== 1 ? 's' : ''}
				</span>
			{/if}
		</div>
		{#if canRemove}
			<button
				onclick={(e) => {
					e.stopPropagation();
					onRemove();
				}}
				class="text-sm text-red-600 hover:text-red-800 px-3 py-1 hover:bg-red-50 rounded-md transition-colors"
			>
				Remove
			</button>
		{/if}
	</div>

	<!-- Content -->
	{#if isExpanded}
		<div class="p-4 space-y-6">
			<!-- Virtual Host Name -->
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-2">
					Virtual Host Name <span class="text-red-500">*</span>
				</label>
				<input
					type="text"
					value={virtualHost.name}
					onchange={(e) => {
						const updated = {
							...virtualHost,
							name: e.currentTarget.value
						};
						onUpdate(updated);
					}}
					placeholder="e.g., api-vhost, web-vhost"
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
				<p class="text-xs text-gray-500 mt-1">
					A unique identifier for this virtual host (lowercase alphanumeric with dashes)
				</p>
			</div>

			<!-- Domains Section -->
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-2">
					Domains <span class="text-red-500">*</span>
				</label>
				<div class="space-y-2">
					{#each virtualHost.domains as domain, domainIndex}
						<div class="flex gap-2">
							<input
								type="text"
								value={domain}
								onchange={(e) => {
									const updated = {
										...virtualHost,
										domains: virtualHost.domains.map((d, i) =>
											i === domainIndex ? e.currentTarget.value : d
										)
									};
									onUpdate(updated);
								}}
								placeholder="e.g., api.example.com"
								class="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							<button
								onclick={() => handleRemoveDomain(domainIndex)}
								class="px-3 py-2 text-sm text-red-600 border border-red-300 rounded-md hover:bg-red-50 transition-colors"
							>
								Remove
							</button>
						</div>
					{/each}
					<div class="flex gap-2">
						<input
							type="text"
							bind:value={newDomain}
							onkeydown={(e) => {
								if (e.key === 'Enter') {
									e.preventDefault();
									handleAddDomain();
								}
							}}
							placeholder="Add a domain..."
							class="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<button
							onclick={handleAddDomain}
							class="px-4 py-2 text-sm text-blue-600 border border-blue-600 rounded-md hover:bg-blue-50 transition-colors"
						>
							<Plus class="h-4 w-4 inline mr-1" />
							Add
						</button>
					</div>
				</div>
				<p class="text-xs text-gray-500 mt-1">
					Add domains that this virtual host should match (e.g., api.example.com, *.api.example.com)
				</p>
			</div>

			<!-- Virtual Host Filters Section -->
			{#if onAddVirtualHostFilter}
				<div class="bg-emerald-50 border border-emerald-200 rounded-lg p-4">
					<div class="flex items-center justify-between mb-3">
						<div class="flex items-center gap-2">
							<Filter class="w-4 h-4 text-emerald-600" />
							<span class="text-sm font-medium text-emerald-800">Virtual Host Filters</span>
							<span class="text-xs text-emerald-600">(apply to all routes in this host)</span>
						</div>
						<button
							onclick={() => onAddVirtualHostFilter?.(virtualHost.name)}
							class="px-3 py-1.5 text-sm text-emerald-700 border border-emerald-300 rounded-md hover:bg-emerald-100 transition-colors"
						>
							<Plus class="h-4 w-4 inline mr-1" />
							Add Filter
						</button>
					</div>

					{#if virtualHostFilters.length > 0}
						<div class="space-y-2">
							{#each virtualHostFilters as filter, filterIndex}
								<div class="flex items-center justify-between p-3 bg-white border border-emerald-200 rounded-lg">
									<div class="flex items-center gap-3">
										<span class="text-xs font-medium text-gray-400">{filterIndex + 1}</span>
										<span class="text-sm font-medium text-gray-900">{filter.name}</span>
										<span class="inline-flex items-center px-2 py-0.5 text-xs font-medium rounded {getFilterTypeBadgeColor(filter.filterType)}">
											{getFilterTypeLabel(filter.filterType)}
										</span>
									</div>
									<button
										onclick={() => onDetachVirtualHostFilter?.(virtualHost.name, filter.id)}
										class="p-1.5 text-red-600 hover:bg-red-50 rounded-md transition-colors"
										title="Detach filter"
									>
										<Trash2 class="h-4 w-4" />
									</button>
								</div>
							{/each}
						</div>
					{:else}
						<p class="text-sm text-emerald-700">No filters attached to this virtual host</p>
					{/if}
				</div>
			{/if}

			<!-- Routes Section -->
			<div>
				<div class="flex items-center justify-between mb-2">
					<label class="block text-sm font-medium text-gray-700">
						Routes <span class="text-red-500">*</span>
					</label>
					<button
						onclick={handleAddRoute}
						class="text-sm text-blue-600 hover:text-blue-800 flex items-center gap-1"
					>
						<Plus class="h-4 w-4" />
						Add Route
					</button>
				</div>

				{#if virtualHost.routes.length === 0}
					<div class="border-2 border-dashed border-gray-300 rounded-lg p-8 text-center">
						<p class="text-sm text-gray-600 mb-3">No routes defined yet</p>
						<button
							onclick={handleAddRoute}
							class="px-4 py-2 text-sm text-blue-600 border border-blue-600 rounded-md hover:bg-blue-50 transition-colors"
						>
							<Plus class="h-4 w-4 inline mr-1" />
							Add First Route
						</button>
					</div>
				{:else}
					<div class="space-y-3">
						{#each virtualHost.routes as route}
							{@const routeFilterList = routeFilters.get(route.name) ?? []}
							{@const hasRouteFilters = routeFilterList.length > 0}
							{@const totalFilters = (routeFilterList.length) + (getInheritedFiltersForRoute().length)}
							<div class="border {hasRouteFilters ? 'border-amber-200 bg-amber-50/50' : 'border-gray-200 bg-gray-50'} rounded-md p-3">
								<div class="flex items-center justify-between mb-3">
									<span class="text-sm font-medium text-gray-900">
										{route.method} {route.path}
									</span>
									<div class="flex items-center gap-2">
										{#if onAddRouteFilter && !hasRouteFilters}
											<button
												onclick={() => onAddRouteFilter?.(virtualHost.name, route.name)}
												class="text-xs text-amber-600 hover:text-amber-800 hover:bg-amber-50 px-2 py-1 rounded transition-colors"
												title="Add route-specific filter"
											>
												<Plus class="h-3.5 w-3.5 inline mr-1" />
												Filter
											</button>
										{/if}
										<button
											onclick={() => toggleRouteAdvanced(route.id)}
											class="text-xs text-blue-600 hover:text-blue-800"
											title="Toggle advanced settings"
										>
											{expandedRoutes.has(route.id) ? 'Hide' : 'Advanced'}
										</button>
										<button
											onclick={() => handleRemoveRoute(route.id)}
											class="text-xs text-red-600 hover:text-red-800"
											title="Remove route"
										>
											<Trash2 class="h-4 w-4" />
										</button>
									</div>
								</div>

								<!-- Basic Route Fields -->
								<!-- Route Name (Full Width) -->
								<div class="mb-3">
									<label class="block text-xs font-medium text-gray-700 mb-1">Route Name</label>
									<input
										type="text"
										value={route.name}
										onchange={(e) => handleUpdateRoute(route.id, 'name', e.currentTarget.value)}
										placeholder="e.g., get-users, create-order"
										class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									/>
									<p class="text-xs text-gray-500 mt-1">A unique identifier for this route (lowercase alphanumeric with dashes)</p>
								</div>

								<div class="grid grid-cols-4 gap-3 mb-3">
									<!-- Method -->
									<div>
										<label class="block text-xs font-medium text-gray-700 mb-1">Method</label>
										<select
											value={route.method}
											onchange={(e) => handleUpdateRoute(route.id, 'method', e.currentTarget.value)}
											class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
										>
											<option value="GET">GET</option>
											<option value="POST">POST</option>
											<option value="PUT">PUT</option>
											<option value="DELETE">DELETE</option>
											<option value="PATCH">PATCH</option>
											<option value="HEAD">HEAD</option>
											<option value="OPTIONS">OPTIONS</option>
										</select>
									</div>

									<!-- Path -->
									<div>
										<label class="block text-xs font-medium text-gray-700 mb-1">Path</label>
										<input
											type="text"
											value={route.path}
											onchange={(e) => handleUpdateRoute(route.id, 'path', e.currentTarget.value)}
											placeholder="/api/v1/users"
											class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
										/>
									</div>

									<!-- Path Type -->
									<div>
										<label class="block text-xs font-medium text-gray-700 mb-1">Match Type</label>
										<select
											value={route.pathType}
											onchange={(e) => handleUpdateRoute(route.id, 'pathType', e.currentTarget.value)}
											class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
										>
											<option value="prefix">Prefix</option>
											<option value="exact">Exact</option>
											<option value="template">Template</option>
											<option value="regex">Regex</option>
										</select>
									</div>

									<!-- Cluster -->
									<div>
										<label class="block text-xs font-medium text-gray-700 mb-1">Cluster</label>
										<select
											value={route.cluster}
											onchange={(e) => handleUpdateRoute(route.id, 'cluster', e.currentTarget.value)}
											class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
										>
											{#if availableClusters.length === 0}
												<option value="">No clusters available</option>
											{:else}
												{#each availableClusters as cluster}
													<option value={cluster}>{cluster}</option>
												{/each}
											{/if}
										</select>
									</div>
								</div>

								<!-- Route Filters Section (Collapsible) -->
								<div class="border-t border-gray-200 pt-3 mt-3">
									<button
										onclick={() => toggleRouteFiltersExpanded(route.id)}
										class="w-full flex items-center justify-between text-left py-1"
									>
										<div class="flex items-center gap-2">
											<Filter class="h-4 w-4 text-amber-600" />
											<span class="text-sm font-medium text-amber-800">Route Filters</span>
											{#if totalFilters > 0}
												<span class="px-2 py-0.5 text-xs rounded-full bg-amber-100 text-amber-700">
													{totalFilters}
												</span>
											{/if}
										</div>
										{#if routeFiltersExpanded.has(route.id)}
											<ChevronUp class="h-4 w-4 text-gray-500" />
										{:else}
											<ChevronDown class="h-4 w-4 text-gray-500" />
										{/if}
									</button>

									{#if routeFiltersExpanded.has(route.id)}
										<div class="mt-3 pl-4 border-l-2 border-amber-200">
											<RouteFilterSection
												{routeConfigName}
												virtualHostName={virtualHost.name}
												routeName={route.name}
												directFilters={routeFilterList}
												inheritedFilters={getInheritedFiltersForRoute()}
												onAddFilter={() => onAddRouteFilter?.(virtualHost.name, route.name)}
												onRemoveFilter={(filterId) => onDetachRouteFilter?.(virtualHost.name, route.name, filterId)}
											/>
										</div>
									{/if}
								</div>

								<!-- Advanced Settings (Collapsible) -->
								{#if expandedRoutes.has(route.id)}
									<div class="border-t border-gray-300 pt-3 mt-3 space-y-3">
										<!-- Timeout -->
										<div class="grid grid-cols-2 gap-3">
											<div>
												<label class="block text-xs font-medium text-gray-700 mb-1">Timeout (seconds)</label>
												<input
													type="number"
													value={route.timeout || 30}
													onchange={(e) => handleUpdateRoute(route.id, 'timeout', parseInt(e.currentTarget.value))}
													placeholder="30"
													min="1"
													max="300"
													class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
												/>
											</div>
										</div>

										<!-- Path Rewrites -->
										<div class="bg-blue-50 border border-blue-200 rounded-md p-3">
											<h4 class="text-xs font-semibold text-gray-900 mb-2">Path Rewrite</h4>
											{#if route.pathType === 'template'}
												<!-- Template Rewrite (only for template match type) -->
												<div>
													<label class="block text-xs font-medium text-gray-700 mb-1">Template Rewrite</label>
													<input
														type="text"
														value={route.templateRewrite || ''}
														onchange={(e) => handleUpdateRoute(route.id, 'templateRewrite', e.currentTarget.value || undefined)}
														placeholder={templatePlaceholder}
														class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
													/>
													<p class="text-xs text-gray-500 mt-1">Rewrite using template pattern (e.g., /api/{templatePlaceholder})</p>
												</div>
											{:else}
												<!-- Prefix Rewrite (for prefix, exact, regex match types) -->
												<div>
													<label class="block text-xs font-medium text-gray-700 mb-1">Prefix Rewrite</label>
													<input
														type="text"
														value={route.prefixRewrite || ''}
														onchange={(e) => handleUpdateRoute(route.id, 'prefixRewrite', e.currentTarget.value || undefined)}
														placeholder="/internal/api"
														class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
													/>
													<p class="text-xs text-gray-500 mt-1">Rewrite matched prefix to this value</p>
												</div>
											{/if}
										</div>

										<!-- Retry Policy -->
										<div class="bg-yellow-50 border border-yellow-200 rounded-md p-3">
											<div class="flex items-center justify-between mb-2">
												<h4 class="text-xs font-semibold text-gray-900">Retry Policy</h4>
												<label class="flex items-center cursor-pointer">
													<input
														type="checkbox"
														checked={route.retryEnabled || false}
														onchange={(e) => handleUpdateRoute(route.id, 'retryEnabled', e.currentTarget.checked)}
														class="rounded border-gray-300 text-blue-600 focus:ring-blue-500 text-sm"
													/>
													<span class="ml-2 text-xs text-gray-700">Enable Retries</span>
												</label>
											</div>

											{#if route.retryEnabled}
												<!-- Quick Presets -->
												<div class="mb-3 pb-3 border-b border-yellow-300">
													<p class="text-xs text-gray-600 mb-2">Quick Presets:</p>
													<div class="flex gap-2">
														<button
															type="button"
															onclick={() => applyRetryPreset(route.id, 'conservative')}
															class="px-3 py-1 text-xs bg-white border border-yellow-300 rounded hover:bg-yellow-100 transition-colors"
															title="2 retries, 5s timeout, basic conditions"
														>
															Conservative
														</button>
														<button
															type="button"
															onclick={() => applyRetryPreset(route.id, 'balanced')}
															class="px-3 py-1 text-xs bg-white border border-yellow-300 rounded hover:bg-yellow-100 transition-colors"
															title="3 retries, 10s timeout, common conditions"
														>
															Balanced
														</button>
														<button
															type="button"
															onclick={() => applyRetryPreset(route.id, 'aggressive')}
															class="px-3 py-1 text-xs bg-white border border-yellow-300 rounded hover:bg-yellow-100 transition-colors"
															title="5 retries, 15s timeout, all conditions"
														>
															Aggressive
														</button>
													</div>
												</div>
											{/if}

											{#if route.retryEnabled}
												<div class="space-y-2">
													<div class="grid grid-cols-3 gap-2">
														<div>
															<label class="block text-xs font-medium text-gray-700 mb-1">Max Retries</label>
															<input
																type="number"
																value={route.maxRetries || 3}
																onchange={(e) => handleUpdateRoute(route.id, 'maxRetries', parseInt(e.currentTarget.value))}
																min="1"
																max="10"
																class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
															/>
														</div>
														<div>
															<label class="block text-xs font-medium text-gray-700 mb-1">Per-Try Timeout (s)</label>
															<input
																type="number"
																value={route.perTryTimeout || 10}
																onchange={(e) => handleUpdateRoute(route.id, 'perTryTimeout', parseInt(e.currentTarget.value))}
																min="1"
																max="60"
																class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
															/>
														</div>
													</div>

													<div>
														<label class="block text-xs font-medium text-gray-700 mb-1">Retry On (Conditions)</label>
														<div class="grid grid-cols-3 gap-2 text-xs">
															{#each ['5xx', 'reset', 'connect-failure', 'retriable-4xx', 'refused-stream', 'gateway-error'] as condition}
																<label class="flex items-center cursor-pointer">
																	<input
																		type="checkbox"
																		checked={route.retryOn?.includes(condition) || false}
																		onchange={(e) => {
																			const current = route.retryOn || ['5xx', 'reset', 'connect-failure'];
																			const updated = e.currentTarget.checked
																				? [...current, condition]
																				: current.filter(c => c !== condition);
																			handleUpdateRoute(route.id, 'retryOn', updated);
																		}}
																		class="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
																	/>
																	<span class="ml-1 text-gray-700">{condition}</span>
																</label>
															{/each}
														</div>
													</div>

													<div class="grid grid-cols-2 gap-2">
														<div>
															<label class="block text-xs font-medium text-gray-700 mb-1">Backoff Base (ms)</label>
															<input
																type="number"
																value={route.backoffBaseMs || 100}
																onchange={(e) => handleUpdateRoute(route.id, 'backoffBaseMs', parseInt(e.currentTarget.value))}
																min="10"
																max="10000"
																class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
															/>
														</div>
														<div>
															<label class="block text-xs font-medium text-gray-700 mb-1">Backoff Max (ms)</label>
															<input
																type="number"
																value={route.backoffMaxMs || 1000}
																onchange={(e) => handleUpdateRoute(route.id, 'backoffMaxMs', parseInt(e.currentTarget.value))}
																min="100"
																max="60000"
																class="w-full px-2 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
															/>
														</div>
													</div>
												</div>
											{/if}
										</div>
									</div>
								{/if}
							</div>
						{/each}
					</div>
				{/if}
			</div>
		</div>
	{/if}
</div>
