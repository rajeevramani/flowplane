<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount, onDestroy } from 'svelte';
	import type {
		RouteResponse,
		ClusterResponse,
		ListenerResponse,
		CreateClusterBody,
		CreateRouteBody,
		CreateListenerBody,
		UpdateRouteBody,
		VirtualHostDefinition,
		ListenerFilterChainInput
	} from '$lib/api/types';
	import ListenerFirstSelector, {
		type ListenerFirstConfig
	} from '$lib/components/ListenerFirstSelector.svelte';
	import DomainGroup, { type DomainGroupData } from '$lib/components/DomainGroup.svelte';
	import DomainEditor from '$lib/components/DomainEditor.svelte';
	import RouteEditor from '$lib/components/RouteEditor.svelte';
	import type { RouteRule } from '$lib/components/EditableRoutesTable.svelte';
	import type { ClusterConfig } from '$lib/components/ClusterSelector.svelte';
	import { selectedTeam } from '$lib/stores/team';
	import type { Unsubscriber } from 'svelte/store';

	// Session state - use global team from navigation
	let currentTeam = $state<string>('');
	let unsubscribe: Unsubscriber;
	let isLoading = $state(true);

	// Listener selection state
	let listenerConfig = $state<ListenerFirstConfig>({
		selectedTeam: '',
		listenerMode: 'existing',
		selectedListenerName: null,
		newListenerConfig: { name: '', address: '0.0.0.0', port: 8080 }
	});
	let availableListeners = $state<ListenerResponse[]>([]);

	// Route configuration state
	let currentRouteConfig = $state<RouteResponse | null>(null);
	let availableClusters = $state<ClusterResponse[]>([]);
	let domainGroups = $state<DomainGroupData[]>([]);
	let pendingClusters = $state<CreateClusterBody[]>([]);
	let hasUnsavedChanges = $state(false);

	// Derived value for proper reactivity with $state arrays
	let hasDomainGroups = $derived(domainGroups.length > 0);

	// Combine available clusters with pending clusters for UI display
	let allClusters = $derived([
		...availableClusters,
		...pendingClusters.map(
			(pc) =>
				({
					name: pc.name,
					team: pc.team,
					serviceName: pc.name,
					config: { endpoints: pc.endpoints }
				}) as ClusterResponse
		)
	]);

	// Modal state
	let showDomainEditor = $state(false);
	let editingDomainGroup = $state<DomainGroupData | null>(null);
	let showRouteEditor = $state(false);
	let editingRoute = $state<RouteRule | null>(null);
	let editingDomainId = $state<string | null>(null);

	// Submit state
	let isSubmitting = $state(false);
	let error = $state<string | null>(null);
	let success = $state<string | null>(null);

	onMount(() => {
		// Subscribe to global team selection from navigation
		unsubscribe = selectedTeam.subscribe((team) => {
			if (team && team !== currentTeam) {
				currentTeam = team;
				// Sync with listener config
				listenerConfig = { ...listenerConfig, selectedTeam: team };
			}
			isLoading = false;
		});
	});

	onDestroy(() => {
		if (unsubscribe) unsubscribe();
	});

	function handleListenerConfigChange(config: ListenerFirstConfig) {
		listenerConfig = config;
		// Clear domain groups when switching listeners
		if (config.listenerMode === 'new') {
			domainGroups = [];
			currentRouteConfig = null;
		}
	}

	function handleListenersLoaded(listeners: ListenerResponse[]) {
		availableListeners = listeners;
	}

	function handleRouteConfigLoaded(
		routeConfig: RouteResponse | null,
		clusters: ClusterResponse[]
	) {
		currentRouteConfig = routeConfig;
		availableClusters = clusters;

		// Transform route config into domain groups
		// Note: API uses camelCase (virtualHosts), but check both for safety
		const virtualHosts = routeConfig?.config?.virtualHosts || routeConfig?.config?.virtual_hosts;

		if (virtualHosts && Array.isArray(virtualHosts)) {
			domainGroups = virtualHosts.map(
				(vh: { name: string; domains: string[]; routes?: unknown[] }) => ({
					id: crypto.randomUUID(),
					domains: vh.domains || [],
					routes: transformRoutesToRules(vh.routes || [])
				})
			);
		} else {
			domainGroups = [];
		}
		hasUnsavedChanges = false;
	}

	/**
	 * Parse PathMatch from Rust enum format.
	 * Rust serializes PathMatch as: {"Exact": "/path"}, {"Prefix": "/path"}, etc.
	 */
	function parsePathMatch(pathObj: unknown): { pathType: RouteRule['pathType']; pathValue: string } {
		if (!pathObj || typeof pathObj !== 'object') {
			return { pathType: 'prefix', pathValue: '/' };
		}

		const path = pathObj as Record<string, unknown>;

		// Handle Rust enum tuple variant format: {"Exact": "/path"}
		if ('Exact' in path && typeof path.Exact === 'string') {
			return { pathType: 'exact', pathValue: path.Exact };
		}
		if ('Prefix' in path && typeof path.Prefix === 'string') {
			return { pathType: 'prefix', pathValue: path.Prefix };
		}
		if ('Regex' in path && typeof path.Regex === 'string') {
			return { pathType: 'regex', pathValue: path.Regex };
		}
		if ('Template' in path && typeof path.Template === 'string') {
			return { pathType: 'template', pathValue: path.Template };
		}

		// Fallback: try the expected format from UI (type/value/template)
		if ('type' in path) {
			const pathType = path.type as RouteRule['pathType'];
			const pathValue = (path.value as string) || (path.template as string) || '/';
			return { pathType, pathValue };
		}

		return { pathType: 'prefix', pathValue: '/' };
	}

	/**
	 * Parse RouteAction from Rust tagged enum format.
	 * API returns: {"type": "forward", "cluster": "x", "timeoutSeconds": 5}
	 * Or: {"type": "weighted", "clusters": [...], "totalWeight": 100}
	 * Or: {"type": "redirect", "hostRedirect": "...", "pathRedirect": "...", "responseCode": 302}
	 */
	function parseRouteAction(actionObj: unknown): {
		actionType: 'forward' | 'weighted' | 'redirect';
		cluster?: string;
		prefixRewrite?: string;
		templateRewrite?: string;
		timeoutSeconds?: number;
		retryPolicy?: {
			maxRetries: number;
			retryOn: string[];
			perTryTimeoutSeconds?: number;
			backoff?: {
				baseIntervalMs?: number;
				maxIntervalMs?: number;
			};
		};
		weightedClusters?: Array<{ name: string; weight: number }>;
		totalWeight?: number;
		hostRedirect?: string;
		pathRedirect?: string;
		responseCode?: number;
	} {
		if (!actionObj || typeof actionObj !== 'object') {
			return { actionType: 'forward', cluster: '' };
		}

		const action = actionObj as Record<string, unknown>;

		// Handle tagged enum format from API
		if (action.type === 'forward') {
			// Parse retry policy if present
			let retryPolicy = undefined;
			if (action.retryPolicy && typeof action.retryPolicy === 'object') {
				const rp = action.retryPolicy as Record<string, unknown>;
				retryPolicy = {
					maxRetries: typeof rp.maxRetries === 'number' ? rp.maxRetries : 3,
					retryOn: Array.isArray(rp.retryOn) ? rp.retryOn : [],
					perTryTimeoutSeconds: typeof rp.perTryTimeoutSeconds === 'number' ? rp.perTryTimeoutSeconds : undefined,
					backoff: rp.backoff && typeof rp.backoff === 'object' ? {
						baseIntervalMs: typeof (rp.backoff as Record<string, unknown>).baseIntervalMs === 'number'
							? (rp.backoff as Record<string, unknown>).baseIntervalMs as number
							: undefined,
						maxIntervalMs: typeof (rp.backoff as Record<string, unknown>).maxIntervalMs === 'number'
							? (rp.backoff as Record<string, unknown>).maxIntervalMs as number
							: undefined
					} : undefined
				};
			}

			return {
				actionType: 'forward',
				cluster: typeof action.cluster === 'string' ? action.cluster : '',
				prefixRewrite: typeof action.prefixRewrite === 'string' ? action.prefixRewrite : undefined,
				templateRewrite: typeof action.templateRewrite === 'string' ? action.templateRewrite : undefined,
				timeoutSeconds: typeof action.timeoutSeconds === 'number' ? action.timeoutSeconds : undefined,
				retryPolicy
			};
		}

		if (action.type === 'weighted') {
			const clusters = Array.isArray(action.clusters)
				? action.clusters.map((c: { name?: string; weight?: number }) => ({
						name: c.name || '',
						weight: c.weight || 0
				  }))
				: [];
			return {
				actionType: 'weighted',
				weightedClusters: clusters,
				totalWeight: typeof action.totalWeight === 'number' ? action.totalWeight : undefined
			};
		}

		if (action.type === 'redirect') {
			return {
				actionType: 'redirect',
				hostRedirect: typeof action.hostRedirect === 'string' ? action.hostRedirect : undefined,
				pathRedirect: typeof action.pathRedirect === 'string' ? action.pathRedirect : undefined,
				responseCode: typeof action.responseCode === 'number' ? action.responseCode : 302
			};
		}

		// Fallback: Handle untagged format: {"Forward": {"cluster": "x"}}
		if ('Forward' in action && typeof action.Forward === 'object' && action.Forward !== null) {
			const forward = action.Forward as Record<string, unknown>;
			return {
				actionType: 'forward',
				cluster: typeof forward.cluster === 'string' ? forward.cluster : '',
				timeoutSeconds:
					typeof forward.timeout_seconds === 'number' ? forward.timeout_seconds : undefined
			};
		}

		return { actionType: 'forward', cluster: '' };
	}

	function transformRoutesToRules(routes: unknown[]): RouteRule[] {
		return routes.map((r: unknown) => {
			const route = r as {
				name?: string;
				match?: {
					path?: unknown;
					headers?: { name: string; value?: string; regex?: string; present?: boolean }[];
					queryParameters?: {
						name: string;
						value?: string;
						regex?: string;
						present?: boolean;
					}[];
				};
				action?: unknown;
			};
			const match = route.match || {};
			const { pathType, pathValue } = parsePathMatch(match.path);
			const actionData = parseRouteAction(route.action);

			// Extract method from headers if present (looks for :method header)
			const methodHeader = match.headers?.find((h) => h.name === ':method');
			const method = methodHeader?.value || '*';

			// Filter out :method from headers for display
			const displayHeaders = match.headers?.filter((h) => h.name !== ':method') || [];

			return {
				id: crypto.randomUUID(),
				method,
				path: pathValue,
				pathType,
				actionType: actionData.actionType,
				// Forward fields
				cluster: actionData.cluster,
				prefixRewrite: actionData.prefixRewrite,
				templateRewrite: actionData.templateRewrite,
				timeoutSeconds: actionData.timeoutSeconds,
				retryPolicy: actionData.retryPolicy,
				// Weighted fields
				weightedClusters: actionData.weightedClusters,
				totalWeight: actionData.totalWeight,
				// Redirect fields
				hostRedirect: actionData.hostRedirect,
				pathRedirect: actionData.pathRedirect,
				responseCode: actionData.responseCode,
				// Common matchers
				headers: displayHeaders.length > 0 ? displayHeaders : undefined,
				queryParams: match.queryParameters?.length ? match.queryParameters : undefined
			};
		});
	}

	// Domain CRUD
	function handleAddDomain() {
		editingDomainGroup = null;
		showDomainEditor = true;
	}

	function handleEditDomain(groupId: string) {
		editingDomainGroup = domainGroups.find((g) => g.id === groupId) || null;
		showDomainEditor = true;
	}

	function handleDeleteDomain(groupId: string) {
		domainGroups = domainGroups.filter((g) => g.id !== groupId);
		hasUnsavedChanges = true;
	}

	function handleSaveDomain(domains: string[]) {
		if (editingDomainGroup) {
			// Update existing
			domainGroups = domainGroups.map((g) =>
				g.id === editingDomainGroup!.id ? { ...g, domains } : g
			);
		} else {
			// Create new
			domainGroups = [
				...domainGroups,
				{
					id: crypto.randomUUID(),
					domains,
					routes: []
				}
			];
		}
		showDomainEditor = false;
		editingDomainGroup = null;
		hasUnsavedChanges = true;
	}

	// Route CRUD
	function handleAddRoute(groupId: string) {
		editingDomainId = groupId;
		editingRoute = null;
		showRouteEditor = true;
	}

	function handleEditRoute(groupId: string, routeId: string) {
		editingDomainId = groupId;
		const group = domainGroups.find((g) => g.id === groupId);
		editingRoute = group?.routes.find((r) => r.id === routeId) || null;
		showRouteEditor = true;
	}

	function handleDeleteRoute(groupId: string, routeId: string) {
		domainGroups = domainGroups.map((g) => {
			if (g.id === groupId) {
				return { ...g, routes: g.routes.filter((r) => r.id !== routeId) };
			}
			return g;
		});
		hasUnsavedChanges = true;
	}

	function handleSaveRoute(route: RouteRule, newCluster: ClusterConfig | null) {
		if (!editingDomainId) return;

		// If creating a new cluster, add it to pending clusters
		if (newCluster?.mode === 'new' && newCluster.newClusterConfig) {
			// Check if cluster already exists in pending clusters to avoid duplicates
			const alreadyPending = pendingClusters.some(
				(pc) => pc.name === newCluster.newClusterConfig!.name
			);
			if (!alreadyPending) {
				const clusterBody: CreateClusterBody = {
					team: listenerConfig.selectedTeam,
					name: newCluster.newClusterConfig.name,
					endpoints: newCluster.newClusterConfig.endpoints.filter((e) => e.host.trim() !== ''),
					lbPolicy:
						newCluster.newClusterConfig.endpoints.length > 1
							? (newCluster.newClusterConfig.lbPolicy as CreateClusterBody['lbPolicy'])
							: undefined
				};
				pendingClusters = [...pendingClusters, clusterBody];
			}
		}

		// Add or update route in domain group
		domainGroups = domainGroups.map((g) => {
			if (g.id === editingDomainId) {
				if (editingRoute) {
					// Update existing route
					return {
						...g,
						routes: g.routes.map((r) => (r.id === editingRoute!.id ? route : r))
					};
				} else {
					// Add new route
					return { ...g, routes: [...g.routes, route] };
				}
			}
			return g;
		});

		showRouteEditor = false;
		editingRoute = null;
		editingDomainId = null;
		hasUnsavedChanges = true;
	}

	function getEditingDomainName(): string {
		if (!editingDomainId) return '';
		const group = domainGroups.find((g) => g.id === editingDomainId);
		return group?.domains[0] || '';
	}

	// Transform domain groups back to virtual hosts for API
	function buildVirtualHosts(): VirtualHostDefinition[] {
		return domainGroups.map((group) => ({
			name: `vhost-${group.domains[0]}`.replace(/[^a-z0-9-]/gi, '-'),
			domains: group.domains,
			routes: group.routes.map((route) => {
				// Build headers array - include :method if not wildcard
				const headers = [];
				if (route.method !== '*') {
					headers.push({ name: ':method', value: route.method });
				}
				if (route.headers) {
					headers.push(...route.headers);
				}

				// Build action based on action type
				let action;
				const actionType = route.actionType || 'forward';
				if (actionType === 'forward') {
					action = {
						type: 'forward' as const,
						cluster: route.cluster || '',
						timeoutSeconds: route.timeoutSeconds || 15,
						prefixRewrite: route.prefixRewrite || undefined,
						templateRewrite: route.templateRewrite || undefined,
						retryPolicy: route.retryPolicy ? {
							maxRetries: route.retryPolicy.maxRetries,
							retryOn: route.retryPolicy.retryOn,
							perTryTimeoutSeconds: route.retryPolicy.perTryTimeoutSeconds || undefined,
							backoff: route.retryPolicy.backoff ? {
								baseIntervalMs: route.retryPolicy.backoff.baseIntervalMs,
								maxIntervalMs: route.retryPolicy.backoff.maxIntervalMs
							} : undefined
						} : undefined
					};
				} else if (actionType === 'weighted') {
					action = {
						type: 'weighted' as const,
						clusters: (route.weightedClusters || []).map((c) => ({
							name: c.name,
							weight: c.weight
						})),
						totalWeight: route.totalWeight
					};
				} else {
					action = {
						type: 'redirect' as const,
						hostRedirect: route.hostRedirect || undefined,
						pathRedirect: route.pathRedirect || undefined,
						responseCode: route.responseCode || 302
					};
				}

				return {
					name: `route-${route.method}-${route.path}`.replace(/[^a-z0-9-]/gi, '-').toLowerCase(),
					match: {
						path:
							route.pathType === 'template'
								? { type: route.pathType, template: route.path }
								: { type: route.pathType, value: route.path },
						headers: headers.length > 0 ? headers : undefined,
						queryParameters: route.queryParams?.length ? route.queryParams : undefined
					},
					action
				};
			})
		}));
	}

	async function handleSaveAll() {
		error = null;
		success = null;
		isSubmitting = true;

		// Validation
		if (!listenerConfig.selectedTeam) {
			error = 'Please select a team';
			isSubmitting = false;
			return;
		}

		if (domainGroups.length === 0) {
			error = 'Please add at least one domain with routes';
			isSubmitting = false;
			return;
		}

		// Validate all routes have proper targets based on action type
		for (const group of domainGroups) {
			for (const route of group.routes) {
				const actionType = route.actionType || 'forward';
				if (actionType === 'forward' && !route.cluster) {
					error = `Route ${route.method} ${route.path} in ${group.domains[0]} needs a target cluster`;
					isSubmitting = false;
					return;
				}
				if (actionType === 'weighted' && (!route.weightedClusters || route.weightedClusters.length === 0)) {
					error = `Route ${route.method} ${route.path} in ${group.domains[0]} needs at least one weighted cluster`;
					isSubmitting = false;
					return;
				}
				if (actionType === 'redirect' && !route.hostRedirect && !route.pathRedirect) {
					error = `Route ${route.method} ${route.path} in ${group.domains[0]} needs a host or path redirect`;
					isSubmitting = false;
					return;
				}
			}
		}

		const createdClusterNames: string[] = [];
		let createdRouteName: string | null = null;

		try {
			// 1. Create any pending clusters
			for (const clusterBody of pendingClusters) {
				await apiClient.createCluster(clusterBody);
				createdClusterNames.push(clusterBody.name);
			}

			const virtualHosts = buildVirtualHosts();

			if (listenerConfig.listenerMode === 'existing' && currentRouteConfig) {
				// Update existing route config
				const updateBody: UpdateRouteBody = {
					team: currentRouteConfig.team,
					name: currentRouteConfig.name,
					virtualHosts
				};
				await apiClient.updateRoute(currentRouteConfig.name, updateBody);
			} else {
				// Create new route config and listener
				const routeName = `${listenerConfig.newListenerConfig.name}-routes`;
				const routeBody: CreateRouteBody = {
					team: listenerConfig.selectedTeam,
					name: routeName,
					virtualHosts
				};
				await apiClient.createRoute(routeBody);
				createdRouteName = routeName;

				// Create new listener
				const listenerBody: CreateListenerBody = {
					team: listenerConfig.selectedTeam,
					name: listenerConfig.newListenerConfig.name,
					address: listenerConfig.newListenerConfig.address,
					port: listenerConfig.newListenerConfig.port,
					protocol: 'HTTP',
					filterChains: [
						{
							name: 'default',
							filters: [
								{
									name: 'envoy.filters.network.http_connection_manager',
									type: 'httpConnectionManager',
									routeConfigName: routeName,
									httpFilters: [{ filter: { type: 'router' } }]
								}
							]
						}
					]
				};
				await apiClient.createListener(listenerBody);
			}

			pendingClusters = [];
			hasUnsavedChanges = false;
			success = 'Routes saved successfully!';

			// Redirect after short delay
			setTimeout(() => {
				goto('/resources');
			}, 1500);
		} catch (e: unknown) {
			// Rollback: delete created clusters
			for (const name of createdClusterNames) {
				try {
					await apiClient.deleteCluster(name);
				} catch {
					// Ignore cleanup errors
				}
			}
			// Rollback: delete created route if applicable
			if (createdRouteName) {
				try {
					await apiClient.deleteRoute(createdRouteName);
				} catch {
					// Ignore cleanup errors
				}
			}

			error = e instanceof Error ? e.message : 'Failed to save routes';
		} finally {
			isSubmitting = false;
		}
	}

	function handleCancel() {
		if (hasUnsavedChanges) {
			if (!confirm('You have unsaved changes. Are you sure you want to leave?')) {
				return;
			}
		}
		goto('/resources');
	}
</script>

<div class="max-w-5xl mx-auto">
	<div class="flex items-center gap-4 mb-6">
		<a href="/resources" class="text-blue-600 hover:text-blue-800">
			<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					stroke-width="2"
					d="M10 19l-7-7m0 0l7-7m-7 7h18"
				/>
			</svg>
		</a>
		<h1 class="text-2xl font-bold text-gray-900">Create/Manage API Routes</h1>
	</div>

	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<svg class="animate-spin h-8 w-8 text-blue-600" fill="none" viewBox="0 0 24 24">
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
		</div>
	{:else}
		<div class="bg-white rounded-lg shadow-md p-6 space-y-8">
			<!-- Step 1: Listener Selection (team from global navigation) -->
			<ListenerFirstSelector
				teams={currentTeam ? [currentTeam] : []}
				config={listenerConfig}
				onConfigChange={handleListenerConfigChange}
				onRouteConfigLoaded={handleRouteConfigLoaded}
				onListenersLoaded={handleListenersLoaded}
			/>

			<!-- Step 2: Route Configuration (visible when listener is selected) -->
			{#if (listenerConfig.listenerMode === 'existing' && listenerConfig.selectedListenerName) || listenerConfig.listenerMode === 'new'}
				<div class="border-t border-gray-200 pt-6">
					<div class="flex items-center justify-between mb-4">
						<h3 class="text-lg font-medium text-gray-900">Step 2: Route Configuration</h3>
						<button
							type="button"
							onclick={handleAddDomain}
							class="flex items-center gap-1 px-3 py-1.5 text-sm font-medium text-blue-600 border border-blue-600 rounded-md hover:bg-blue-50"
						>
							<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path
									stroke-linecap="round"
									stroke-linejoin="round"
									stroke-width="2"
									d="M12 4v16m8-8H4"
								/>
							</svg>
							Add Domain
						</button>
					</div>

					{#if !hasDomainGroups}
						<div class="text-center py-12 bg-gray-50 rounded-lg border-2 border-dashed border-gray-300">
							<svg
								class="mx-auto h-12 w-12 text-gray-400"
								fill="none"
								stroke="currentColor"
								viewBox="0 0 24 24"
							>
								<path
									stroke-linecap="round"
									stroke-linejoin="round"
									stroke-width="2"
									d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10"
								/>
							</svg>
							<h3 class="mt-2 text-sm font-medium text-gray-900">No domains configured</h3>
							<p class="mt-1 text-sm text-gray-500">
								Get started by adding a domain to configure routes.
							</p>
							<div class="mt-6">
								<button
									type="button"
									onclick={handleAddDomain}
									class="inline-flex items-center gap-1 px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
								>
									<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
										<path
											stroke-linecap="round"
											stroke-linejoin="round"
											stroke-width="2"
											d="M12 4v16m8-8H4"
										/>
									</svg>
									Add Domain
								</button>
							</div>
						</div>
					{:else}
						<div class="space-y-4">
							{#each domainGroups as group}
								<DomainGroup
									{group}
									clusters={allClusters}
									onEditDomain={handleEditDomain}
									onDeleteDomain={handleDeleteDomain}
									onAddRoute={handleAddRoute}
									onEditRoute={handleEditRoute}
									onDeleteRoute={handleDeleteRoute}
								/>
							{/each}
						</div>
					{/if}
				</div>
			{/if}

			<!-- Feedback -->
			{#if error}
				<div class="bg-red-50 border-l-4 border-red-500 p-4 rounded-md">
					<p class="text-red-700">{error}</p>
				</div>
			{/if}
			{#if success}
				<div class="bg-green-50 border-l-4 border-green-500 p-4 rounded-md">
					<p class="text-green-700">{success}</p>
				</div>
			{/if}

			<!-- Actions -->
			<div class="flex justify-end gap-4 pt-4 border-t">
				<button
					type="button"
					onclick={handleCancel}
					class="px-6 py-2 border border-gray-300 text-gray-700 font-medium rounded-md hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-gray-500"
				>
					Cancel
				</button>
				<button
					type="button"
					onclick={handleSaveAll}
					disabled={isSubmitting || (!hasUnsavedChanges && listenerConfig.listenerMode === 'existing')}
					class="px-6 py-2 bg-blue-600 text-white font-medium rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500 disabled:opacity-50"
				>
					{isSubmitting ? 'Saving...' : 'Save All Changes'}
				</button>
			</div>
		</div>
	{/if}
</div>

<!-- Modals -->
<DomainEditor
	show={showDomainEditor}
	group={editingDomainGroup}
	onSave={handleSaveDomain}
	onCancel={() => {
		showDomainEditor = false;
		editingDomainGroup = null;
	}}
/>

<RouteEditor
	show={showRouteEditor}
	route={editingRoute}
	domainName={getEditingDomainName()}
	clusters={allClusters}
	onSave={handleSaveRoute}
	onCancel={() => {
		showRouteEditor = false;
		editingRoute = null;
		editingDomainId = null;
	}}
/>
