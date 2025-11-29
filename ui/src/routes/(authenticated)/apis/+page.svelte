<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import { Plus, Eye, Lock, LockOpen } from 'lucide-svelte';
	import DetailDrawer from '$lib/components/DetailDrawer.svelte';
	import ConfigCard from '$lib/components/ConfigCard.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import Tooltip from '$lib/components/Tooltip.svelte';
	import type { RouteResponse, ClusterResponse, ListenerResponse, ImportSummary } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import type { Unsubscriber } from 'svelte/store';

	interface RouteDetail {
		name: string;
		apiName: string;
		team: string;
		method: string;
		path: string;
		matchType: 'exact' | 'prefix' | 'template' | 'regex';
		cluster: string;
		timeout?: number;
		prefixRewrite?: string;
		templateRewrite?: string;
		retryPolicy?: {
			numRetries?: number;
			retryOn?: string;
			perTryTimeout?: string;
			retryBackOff?: {
				baseInterval?: string;
				maxInterval?: string;
			};
		};
		sourceRoute: RouteResponse;
	}

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');
	let unsubscribe: Unsubscriber;

	// Data
	let routes = $state<RouteResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);
	let listeners = $state<ListenerResponse[]>([]);
	let imports = $state<ImportSummary[]>([]);

	// Drawer state
	let drawerOpen = $state(false);
	let selectedRoute = $state<RouteResponse | null>(null);

	onMount(async () => {
		unsubscribe = selectedTeam.subscribe(async (team) => {
			if (team && team !== currentTeam) {
				currentTeam = team;
				await loadData();
			}
		});
	});

	onDestroy(() => {
		if (unsubscribe) unsubscribe();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		try {
			const [routesData, clustersData, listenersData, importsData] = await Promise.all([
				apiClient.listRoutes(),
				apiClient.listClusters(),
				apiClient.listListeners(),
				currentTeam ? apiClient.listImports(currentTeam) : Promise.resolve([])
			]);

			routes = routesData;
			clusters = clustersData;
			listeners = listenersData;
			imports = importsData;

			// Console log the route data for debugging
			console.log('Routes data:', routesData);
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Extract and flatten all route details from all routes
	function extractAllRouteDetails(routes: RouteResponse[]): RouteDetail[] {
		const allDetails: RouteDetail[] = [];

		for (const route of routes) {
			const config = route.config;

			if (config?.virtualHosts) {
				for (const vh of config.virtualHosts) {
					for (const r of vh.routes || []) {
						const methodHeader = r.match?.headers?.find((h: { name: string }) => h.name === ':method');
						const method = methodHeader?.value || '*';

						const pathMatch = r.match?.path;
						let path = '';
						let matchType: RouteDetail['matchType'] = 'exact';

						if (pathMatch) {
							matchType = pathMatch.type || 'exact';
							path = pathMatch.value || pathMatch.template || '';
						}

						// Handle Rust enum serialization
						const clusterAction = r.action?.Cluster || r.action;
						const weightedAction = r.action?.WeightedClusters;

						// Extract cluster name (handle both 'name' and 'cluster' field names)
						let cluster = '';
						if (clusterAction?.name) {
							cluster = clusterAction.name;
						} else if (clusterAction?.cluster) {
							cluster = clusterAction.cluster;
						} else if (weightedAction?.clusters) {
							cluster = weightedAction.clusters.map((c: { name: string }) => c.name).join(', ');
						} else if (r.route?.cluster) {
							cluster = r.route.cluster;
						}

						// Extract rewrite info
						const prefixRewrite = clusterAction?.prefix_rewrite || clusterAction?.prefixRewrite || r.route?.prefixRewrite;
						const templateRewrite = clusterAction?.path_template_rewrite || clusterAction?.templateRewrite || r.route?.regexRewrite?.substitution;

						// Extract retry policy
						const rawRetryPolicy = clusterAction?.retry_policy || clusterAction?.retryPolicy || r.route?.retryPolicy;
						let retryPolicy: RouteDetail['retryPolicy'] | undefined;

						if (rawRetryPolicy) {
							const retryOn = rawRetryPolicy.retry_on || rawRetryPolicy.retryOn;
							const retryOnStr = Array.isArray(retryOn) ? retryOn.join(', ') : retryOn;

							// Handle perTryTimeout from multiple possible field names
							let perTryTimeout: string | undefined;
							if (rawRetryPolicy.per_try_timeout_seconds) {
								perTryTimeout = `${rawRetryPolicy.per_try_timeout_seconds}s`;
							} else if (rawRetryPolicy.perTryTimeoutSeconds) {
								perTryTimeout = `${rawRetryPolicy.perTryTimeoutSeconds}s`;
							} else if (rawRetryPolicy.perTryTimeout) {
								perTryTimeout = rawRetryPolicy.perTryTimeout;
							}

							// Handle backoff from multiple possible structures
							let retryBackOff: RouteDetail['retryPolicy']['retryBackOff'] | undefined;
							if (rawRetryPolicy.base_interval_ms || rawRetryPolicy.max_interval_ms) {
								retryBackOff = {
									baseInterval: rawRetryPolicy.base_interval_ms ? `${rawRetryPolicy.base_interval_ms}ms` : undefined,
									maxInterval: rawRetryPolicy.max_interval_ms ? `${rawRetryPolicy.max_interval_ms}ms` : undefined
								};
							} else if (rawRetryPolicy.backoff) {
								// Handle camelCase backoff object from API
								retryBackOff = {
									baseInterval: rawRetryPolicy.backoff.baseIntervalMs ? `${rawRetryPolicy.backoff.baseIntervalMs}ms` : undefined,
									maxInterval: rawRetryPolicy.backoff.maxIntervalMs ? `${rawRetryPolicy.backoff.maxIntervalMs}ms` : undefined
								};
							} else if (rawRetryPolicy.retryBackOff || rawRetryPolicy.retry_back_off) {
								retryBackOff = rawRetryPolicy.retryBackOff || rawRetryPolicy.retry_back_off;
							}

							retryPolicy = {
								numRetries: rawRetryPolicy.num_retries ?? rawRetryPolicy.numRetries ?? rawRetryPolicy.maxRetries,
								retryOn: retryOnStr,
								perTryTimeout,
								retryBackOff
							};
						}

						// Extract timeout
						const timeout = clusterAction?.timeout ?? clusterAction?.timeoutSeconds ?? r.route?.timeout;

						allDetails.push({
							name: r.name,
							apiName: route.name,
							team: route.team,
							method,
							path,
							matchType,
							cluster,
							timeout,
							prefixRewrite,
							templateRewrite,
							retryPolicy,
							sourceRoute: route
						});
					}
				}
			}
		}

		return allDetails;
	}

	// Filter routes by team and search, then flatten
	let filteredRouteDetails = $derived(() => {
		const teamFiltered = routes.filter((route) => {
			if (currentTeam && route.team !== currentTeam) return false;
			return true;
		});

		const allDetails = extractAllRouteDetails(teamFiltered);

		if (!searchQuery) return allDetails;

		const query = searchQuery.toLowerCase();
		return allDetails.filter((detail) =>
			detail.apiName.toLowerCase().includes(query) ||
			detail.team.toLowerCase().includes(query) ||
			detail.path.toLowerCase().includes(query) ||
			detail.cluster.toLowerCase().includes(query)
		);
	});

	function getClusterNamesForRoute(route: RouteResponse): Set<string> {
		const clusterNames = new Set<string>();
		route.config?.virtualHosts?.forEach((vh: { routes?: unknown[] }) => {
			vh.routes?.forEach((r: unknown) => {
				const route = r as { action?: { Cluster?: { name?: string }, WeightedClusters?: { clusters?: { name: string }[] }, cluster?: string }, route?: { cluster?: string } };
				const clusterAction = route.action?.Cluster;
				const weightedAction = route.action?.WeightedClusters;

				if (clusterAction?.name) {
					clusterNames.add(clusterAction.name);
				} else if (weightedAction?.clusters) {
					weightedAction.clusters.forEach((c) => clusterNames.add(c.name));
				} else if (route.action?.cluster) {
					clusterNames.add(route.action.cluster);
				} else if (route.route?.cluster) {
					clusterNames.add(route.route.cluster);
				}
			});
		});
		return clusterNames;
	}

	function getMethodBadgeVariant(method: string): 'green' | 'blue' | 'yellow' | 'red' | 'gray' {
		switch (method.toUpperCase()) {
			case 'GET': return 'green';
			case 'POST': return 'blue';
			case 'PUT':
			case 'PATCH': return 'yellow';
			case 'DELETE': return 'red';
			default: return 'gray';
		}
	}

	function getMatchTypeLabel(matchType: string): string {
		switch (matchType) {
			case 'exact': return 'Exact';
			case 'prefix': return 'Prefix';
			case 'template': return 'Template';
			case 'regex': return 'Regex';
			default: return matchType;
		}
	}

	function truncateText(text: string, maxLength: number = 25): string {
		if (text.length <= maxLength) return text;
		return text.substring(0, maxLength) + '...';
	}

	function formatRewrite(detail: RouteDetail): string | null {
		if (detail.prefixRewrite) return detail.prefixRewrite;
		if (detail.templateRewrite) return detail.templateRewrite;
		return null;
	}

	function formatRetryOn(retryOn: string | undefined): string {
		if (!retryOn) return '-';
		return retryOn.split(',').map((s) => s.trim()).join(', ');
	}

	function formatBackoff(policy: RouteDetail['retryPolicy']): string {
		if (!policy?.retryBackOff) return '-';
		const base = policy.retryBackOff.baseInterval || '25ms';
		const max = policy.retryBackOff.maxInterval || '250ms';
		return `${base} - ${max}`;
	}

	function openDrawer(route: RouteResponse) {
		selectedRoute = route;
		drawerOpen = true;
	}

	function closeDrawer() {
		drawerOpen = false;
		selectedRoute = null;
	}

	async function handleDelete(routeName: string) {
		if (!confirm(`Are you sure you want to delete the API "${routeName}"?`)) return;

		try {
			await apiClient.deleteRoute(routeName);
			await loadData();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete route';
		}
	}

	function getListenerForRoute(route: RouteResponse): ListenerResponse | undefined {
		return listeners.find((l) =>
			l.config?.filter_chains?.some((fc: { filters?: { filter_type?: { HttpConnectionManager?: { route_config_name?: string } } }[] }) =>
				fc.filters?.some((f) => f.filter_type?.HttpConnectionManager?.route_config_name === route.name)
			)
		);
	}

	function getRetryPoliciesForRoute(route: RouteResponse): Array<{ routeName: string; policy: RouteDetail['retryPolicy'] }> {
		const policies: Array<{ routeName: string; policy: RouteDetail['retryPolicy'] }> = [];

		if (route.config?.virtualHosts) {
			for (const vh of route.config.virtualHosts) {
				for (const r of vh.routes || []) {
					const clusterAction = r.action?.Cluster || r.action;
					const rawRetryPolicy = clusterAction?.retry_policy || clusterAction?.retryPolicy;

					if (rawRetryPolicy) {
						const retryOn = rawRetryPolicy.retry_on || rawRetryPolicy.retryOn;
						const retryOnStr = Array.isArray(retryOn) ? retryOn.join(', ') : retryOn;

						let perTryTimeout: string | undefined;
						if (rawRetryPolicy.per_try_timeout_seconds) {
							perTryTimeout = `${rawRetryPolicy.per_try_timeout_seconds}s`;
						} else if (rawRetryPolicy.perTryTimeoutSeconds) {
							perTryTimeout = `${rawRetryPolicy.perTryTimeoutSeconds}s`;
						} else if (rawRetryPolicy.perTryTimeout) {
							perTryTimeout = rawRetryPolicy.perTryTimeout;
						}

						let retryBackOff: RouteDetail['retryPolicy']['retryBackOff'] | undefined;
						if (rawRetryPolicy.backoff) {
							retryBackOff = {
								baseInterval: rawRetryPolicy.backoff.baseIntervalMs ? `${rawRetryPolicy.backoff.baseIntervalMs}ms` : undefined,
								maxInterval: rawRetryPolicy.backoff.maxIntervalMs ? `${rawRetryPolicy.backoff.maxIntervalMs}ms` : undefined
							};
						} else if (rawRetryPolicy.base_interval_ms || rawRetryPolicy.max_interval_ms) {
							retryBackOff = {
								baseInterval: rawRetryPolicy.base_interval_ms ? `${rawRetryPolicy.base_interval_ms}ms` : undefined,
								maxInterval: rawRetryPolicy.max_interval_ms ? `${rawRetryPolicy.max_interval_ms}ms` : undefined
							};
						}

						policies.push({
							routeName: r.name || 'unnamed',
							policy: {
								numRetries: rawRetryPolicy.num_retries ?? rawRetryPolicy.numRetries ?? rawRetryPolicy.maxRetries,
								retryOn: retryOnStr,
								perTryTimeout,
								retryBackOff
							}
						});
					}
				}
			}
		}

		return policies;
	}

	function getClustersForRoute(route: RouteResponse): ClusterResponse[] {
		const clusterNames = getClusterNamesForRoute(route);
		return clusters.filter((c) => clusterNames.has(c.name));
	}

	function getUniqueRowId(detail: RouteDetail, index: number): string {
		return `${detail.apiName}-${detail.name || index}-${detail.method}-${detail.path}`;
	}

	// ============================================
	// Cluster Configuration Extraction Functions
	// ============================================

	interface ClusterEndpoint {
		address: string;
		port: number;
	}

	interface HealthCheckInfo {
		type: string;
		path?: string;
		interval: string;
		timeout: string;
		healthyThreshold?: number;
		unhealthyThreshold?: number;
	}

	interface CircuitBreakerInfo {
		maxConnections: number;
		maxPendingRequests: number;
		maxRequests: number;
		maxRetries: number;
	}

	interface OutlierDetectionInfo {
		consecutive5xx: number;
		interval: string;
		baseEjectionTime: string;
		maxEjectionPercent: number;
	}

	interface ClusterTlsInfo {
		serverName?: string;
		verifyCertificate?: boolean;
		minTlsVersion?: string;
	}

	function extractClusterEndpoints(cluster: ClusterResponse): ClusterEndpoint[] {
		const config = cluster.config || {};
		const endpoints: ClusterEndpoint[] = [];
		const lbEndpoints = config.loadAssignment?.endpoints?.[0]?.lbEndpoints || [];

		for (const ep of lbEndpoints) {
			const addr = ep.endpoint?.address?.socketAddress;
			if (addr) {
				endpoints.push({
					address: addr.address,
					port: addr.portValue
				});
			}
		}

		return endpoints;
	}

	function extractLoadBalancingPolicy(config: Record<string, unknown>): { policy: string; params?: string } {
		const policy = (config.lbPolicy as string) || 'ROUND_ROBIN';
		let params: string | undefined;

		// Extract LB-specific params
		if (policy === 'LEAST_REQUEST' && config.leastRequestLbConfig) {
			const lrConfig = config.leastRequestLbConfig as { choiceCount?: number };
			params = `Choice count: ${lrConfig.choiceCount || 2}`;
		} else if (policy === 'RING_HASH' && config.ringHashLbConfig) {
			const rhConfig = config.ringHashLbConfig as { minimumRingSize?: number };
			params = `Min ring size: ${rhConfig.minimumRingSize || 1024}`;
		}

		return { policy, params };
	}

	function formatLbPolicy(policy: string): string {
		return policy.replace(/_/g, ' ').toLowerCase().replace(/\b\w/g, (c) => c.toUpperCase());
	}

	function extractHealthChecks(config: Record<string, unknown>): HealthCheckInfo[] {
		const healthChecks: HealthCheckInfo[] = [];
		const hcList = config.healthChecks as Array<{
			httpHealthCheck?: { path?: string };
			tcpHealthCheck?: unknown;
			grpcHealthCheck?: { serviceName?: string };
			interval?: string;
			timeout?: string;
			healthyThreshold?: number;
			unhealthyThreshold?: number;
		}> || [];

		for (const hc of hcList) {
			let type = 'TCP';
			let path: string | undefined;

			if (hc.httpHealthCheck) {
				type = 'HTTP';
				path = hc.httpHealthCheck.path;
			} else if (hc.grpcHealthCheck) {
				type = 'gRPC';
				path = hc.grpcHealthCheck.serviceName;
			}

			healthChecks.push({
				type,
				path,
				interval: hc.interval || '5s',
				timeout: hc.timeout || '5s',
				healthyThreshold: hc.healthyThreshold,
				unhealthyThreshold: hc.unhealthyThreshold
			});
		}

		return healthChecks;
	}

	function extractCircuitBreaker(config: Record<string, unknown>): CircuitBreakerInfo | null {
		const cb = config.circuitBreakers as { thresholds?: Array<{
			maxConnections?: number;
			maxPendingRequests?: number;
			maxRequests?: number;
			maxRetries?: number;
		}> };

		if (!cb?.thresholds?.[0]) return null;

		const thresholds = cb.thresholds[0];
		return {
			maxConnections: thresholds.maxConnections || 1024,
			maxPendingRequests: thresholds.maxPendingRequests || 1024,
			maxRequests: thresholds.maxRequests || 1024,
			maxRetries: thresholds.maxRetries || 3
		};
	}

	function extractOutlierDetection(config: Record<string, unknown>): OutlierDetectionInfo | null {
		const od = config.outlierDetection as {
			consecutive5xx?: number;
			interval?: string;
			baseEjectionTime?: string;
			maxEjectionPercent?: number;
		};

		if (!od) return null;

		return {
			consecutive5xx: od.consecutive5xx || 5,
			interval: od.interval || '10s',
			baseEjectionTime: od.baseEjectionTime || '30s',
			maxEjectionPercent: od.maxEjectionPercent || 10
		};
	}

	function extractClusterTls(config: Record<string, unknown>): ClusterTlsInfo | null {
		const transportSocket = config.transportSocket as {
			typedConfig?: {
				sni?: string;
				commonTlsContext?: {
					tlsParams?: { tlsMinimumProtocolVersion?: string };
					validationContext?: unknown;
				};
			};
		};

		if (!transportSocket?.typedConfig) return null;

		const tlsConfig = transportSocket.typedConfig;
		return {
			serverName: tlsConfig.sni,
			verifyCertificate: Boolean(tlsConfig.commonTlsContext?.validationContext),
			minTlsVersion: tlsConfig.commonTlsContext?.tlsParams?.tlsMinimumProtocolVersion
		};
	}

	// ============================================
	// Listener Configuration Extraction Functions
	// ============================================

	interface ListenerTlsInfo {
		requireClientCert?: boolean;
		minTlsVersion?: string;
		hasCertificate: boolean;
	}

	interface FilterChainInfo {
		name: string;
		filters: string[];
		hasTls: boolean;
		routeConfigName?: string;
	}

	interface TracingInfo {
		provider: string;
		samplingRate?: number;
	}

	interface AccessLogInfo {
		path?: string;
		format?: string;
	}

	function extractListenerTls(listener: ListenerResponse): ListenerTlsInfo | null {
		const config = listener.config || {};
		const filterChains = config.filter_chains as Array<{
			tls_context?: {
				require_client_certificate?: boolean;
				cert_chain_file?: string;
			};
		}> || [];

		// Check if any filter chain has TLS
		for (const fc of filterChains) {
			const tls = fc.tls_context;
			if (tls) {
				return {
					requireClientCert: tls.require_client_certificate,
					minTlsVersion: undefined, // Not stored in our config format
					hasCertificate: Boolean(tls.cert_chain_file)
				};
			}
		}

		return null;
	}

	function extractFilterChains(listener: ListenerResponse): FilterChainInfo[] {
		const config = listener.config || {};
		const filterChains = config.filter_chains as Array<{
			name?: string;
			tls_context?: unknown;
			filters?: Array<{
				name?: string;
				filter_type?: {
					HttpConnectionManager?: { route_config_name?: string };
					TcpProxy?: { cluster?: string };
				};
			}>;
		}> || [];

		return filterChains.map((fc, i) => {
			const filters = (fc.filters || []).map((f) => {
				if (f.name) return f.name;
				if (f.filter_type?.HttpConnectionManager) return 'HttpConnectionManager';
				if (f.filter_type?.TcpProxy) return 'TcpProxy';
				return 'Filter';
			});

			const routeConfigName = fc.filters?.find((f) => f.filter_type?.HttpConnectionManager?.route_config_name)
				?.filter_type?.HttpConnectionManager?.route_config_name;

			return {
				name: fc.name || `Chain ${i + 1}`,
				filters,
				hasTls: Boolean(fc.tls_context),
				routeConfigName
			};
		});
	}

	function extractHttpFilters(listener: ListenerResponse): string[] {
		const config = listener.config || {};
		const filterChains = config.filter_chains as Array<{
			filters?: Array<{
				filter_type?: {
					HttpConnectionManager?: {
						http_filters?: Array<{
							name?: string;
							filter?: { type?: string };
						}>;
					};
				};
			}>;
		}> || [];

		const httpFilters: string[] = [];

		for (const fc of filterChains) {
			for (const filter of fc.filters || []) {
				const hcmFilters = filter.filter_type?.HttpConnectionManager?.http_filters || [];
				for (const hf of hcmFilters) {
					const name = hf.name || hf.filter?.type || 'unknown';
					if (!httpFilters.includes(name) && name !== 'router' && name !== 'envoy.filters.http.router') {
						httpFilters.push(name);
					}
				}
			}
		}

		return httpFilters;
	}

	function extractTracingConfig(listener: ListenerResponse): TracingInfo | null {
		const config = listener.config || {};
		const filterChains = config.filter_chains as Array<{
			filters?: Array<{
				filter_type?: {
					HttpConnectionManager?: {
						tracing?: {
							provider?: { type?: string };
							random_sampling_percentage?: number;
						};
					};
				};
			}>;
		}> || [];

		for (const fc of filterChains) {
			for (const filter of fc.filters || []) {
				const tracing = filter.filter_type?.HttpConnectionManager?.tracing;
				if (tracing) {
					// Extract provider type from the enum structure
					const providerType = tracing.provider?.type || 'Unknown';
					return {
						provider: providerType.replace(/_/g, ' ').replace(/\b\w/g, (c: string) => c.toUpperCase()),
						samplingRate: tracing.random_sampling_percentage
					};
				}
			}
		}

		return null;
	}

	function extractAccessLogConfig(listener: ListenerResponse): AccessLogInfo | null {
		const config = listener.config || {};
		const filterChains = config.filter_chains as Array<{
			filters?: Array<{
				filter_type?: {
					HttpConnectionManager?: {
						access_log?: {
							path?: string;
							format?: string;
						};
					};
					TcpProxy?: {
						access_log?: {
							path?: string;
							format?: string;
						};
					};
				};
			}>;
		}> || [];

		for (const fc of filterChains) {
			for (const filter of fc.filters || []) {
				const accessLog = filter.filter_type?.HttpConnectionManager?.access_log ||
					filter.filter_type?.TcpProxy?.access_log;
				if (accessLog) {
					return {
						path: accessLog.path,
						format: accessLog.format ? 'Custom' : 'Default'
					};
				}
			}
		}

		return null;
	}

	function formatHttpFilterName(name: string): string {
		// Convert envoy filter names to readable format
		return name
			.replace('envoy.filters.http.', '')
			.replace(/_/g, ' ')
			.replace(/\b\w/g, (c) => c.toUpperCase());
	}
</script>

<!-- Page Header -->
<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="text-2xl font-bold text-gray-900">APIs</h1>
		<p class="mt-1 text-sm text-gray-600">Manage your API routes and configurations</p>
	</div>
	<div class="flex gap-3">
		<a
			href="/apis/manage"
			class="inline-flex items-center gap-2 px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-md hover:bg-blue-700 transition-colors"
		>
			<Plus class="h-4 w-4" />
			Manage APIs
		</a>
		<a
			href="/imports/import"
			class="inline-flex items-center gap-2 px-4 py-2 bg-white text-gray-700 text-sm font-medium rounded-md border border-gray-300 hover:bg-gray-50 transition-colors"
		>
			Import OpenAPI
		</a>
	</div>
</div>

<!-- Error Message -->
{#if error}
	<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
		<p class="text-red-800 text-sm">{error}</p>
	</div>
{/if}

<!-- Search Bar -->
<div class="mb-6">
	<input
		type="text"
		bind:value={searchQuery}
		placeholder="Search APIs..."
		class="w-full max-w-md px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
	/>
</div>

<!-- APIs Table -->
<div class="bg-white rounded-lg shadow">
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
		</div>
	{:else if filteredRouteDetails().length === 0}
		<div class="text-center py-12 text-gray-500">
			No APIs found. Create one to get started.
		</div>
	{:else}
		<div class="overflow-x-auto">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Name</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Team</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-20">Method</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Path</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-20">Match</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Cluster</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">Rewrite</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-24">Retries</th>
						<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider w-20">Timeout</th>
						<th class="px-4 py-3 text-center text-xs font-medium text-gray-500 uppercase tracking-wider w-16">Actions</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each filteredRouteDetails() as detail, index (getUniqueRowId(detail, index))}
						{@const rewrite = formatRewrite(detail)}
						{@const rowId = getUniqueRowId(detail, index)}

						<tr class="hover:bg-gray-50 transition-colors">
							<td class="px-4 py-3">
								<button
									onclick={() => openDrawer(detail.sourceRoute)}
									class="font-medium text-blue-600 hover:text-blue-800 hover:underline text-left"
								>
									{truncateText(detail.apiName, 20)}
								</button>
							</td>
							<td class="px-4 py-3">
								<Badge variant="indigo" size="sm">{detail.team}</Badge>
							</td>
							<td class="px-4 py-3">
								<Badge variant={getMethodBadgeVariant(detail.method)} size="sm">
									{detail.method.toUpperCase()}
								</Badge>
							</td>
							<td class="px-4 py-3">
								<code class="text-sm text-gray-800 bg-gray-100 px-2 py-0.5 rounded font-mono" title={detail.path}>
									{truncateText(detail.path || '/', 30)}
								</code>
							</td>
							<td class="px-4 py-3">
								<span class="text-xs bg-blue-100 text-blue-700 px-2 py-0.5 rounded font-medium">
									{getMatchTypeLabel(detail.matchType)}
								</span>
							</td>
							<td class="px-4 py-3">
								<span class="text-sm text-gray-600" title={detail.cluster}>
									{truncateText(detail.cluster, 25)}
								</span>
							</td>
							<td class="px-4 py-3">
								{#if rewrite}
									<span class="text-sm text-gray-600 flex items-center gap-1">
										<span class="text-gray-400">&rarr;</span>
										<code class="font-mono text-xs bg-gray-100 px-1.5 py-0.5 rounded" title={rewrite}>
											{truncateText(rewrite, 20)}
										</code>
									</span>
								{:else}
									<span class="text-gray-400">-</span>
								{/if}
							</td>
							<td class="px-4 py-3">
								{#if detail.retryPolicy && detail.retryPolicy.numRetries}
									<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-orange-100 text-orange-800">
										{detail.retryPolicy.numRetries}
									</span>
								{:else}
									<span class="text-gray-400">-</span>
								{/if}
							</td>
							<td class="px-4 py-3 text-center">
								<span class="text-sm text-gray-500">
									{detail.timeout ? `${detail.timeout}s` : '-'}
								</span>
							</td>
							<td class="px-4 py-3 text-center">
								<button
									onclick={(e) => {
										e.stopPropagation();
										openDrawer(detail.sourceRoute);
									}}
									class="p-1.5 rounded hover:bg-gray-100 text-gray-500 hover:text-blue-600 transition-colors"
									title="View details"
								>
									<Eye class="h-4 w-4" />
								</button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>

<!-- Detail Drawer -->
<DetailDrawer
	open={drawerOpen}
	title={selectedRoute?.name || ''}
	subtitle={selectedRoute ? `Team: ${selectedRoute.team}` : undefined}
	onClose={closeDrawer}
>
	{#if selectedRoute}
		{@const listener = getListenerForRoute(selectedRoute)}
		{@const routeClusters = getClustersForRoute(selectedRoute)}
		{@const listenerTls = listener ? extractListenerTls(listener) : null}
		{@const filterChains = listener ? extractFilterChains(listener) : []}
		{@const httpFilters = listener ? extractHttpFilters(listener) : []}
		{@const tracingConfig = listener ? extractTracingConfig(listener) : null}
		{@const accessLogConfig = listener ? extractAccessLogConfig(listener) : null}

		<div class="space-y-6">
			<!-- Overview -->
			<ConfigCard title="Overview" variant="gray">
				<dl class="grid grid-cols-2 gap-4 text-sm">
					<div>
						<dt class="text-gray-500">Path Prefix</dt>
						<dd class="font-mono text-gray-900">{selectedRoute.pathPrefix}</dd>
					</div>
					<div>
						<dt class="text-gray-500">Cluster Targets</dt>
						<dd class="text-gray-900">{selectedRoute.clusterTargets}</dd>
					</div>
					{#if selectedRoute.importId}
						{@const importRecord = imports.find((i) => i.id === selectedRoute?.importId)}
						<div>
							<dt class="text-gray-500">Source</dt>
							<dd class="text-gray-900">
								{importRecord?.specName || 'Imported'}
								{#if importRecord?.specVersion}
									<span class="text-gray-500">v{importRecord.specVersion}</span>
								{/if}
							</dd>
						</div>
					{/if}
				</dl>
			</ConfigCard>

			<!-- ============================================ -->
			<!-- Enhanced Listener Section -->
			<!-- ============================================ -->
			{#if listener}
				<ConfigCard title="Listener" variant="blue">
					<dl class="grid grid-cols-2 gap-4 text-sm">
						<div>
							<dt class="text-gray-500">Name</dt>
							<dd class="text-gray-900">{listener.name}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Address</dt>
							<dd class="font-mono text-gray-900">{listener.address}:{listener.port}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Protocol</dt>
							<dd class="text-gray-900">{listener.protocol || 'HTTP'}</dd>
						</div>
						<div>
							<dt class="text-gray-500">TLS</dt>
							<dd class="text-gray-900">
								{#if listenerTls}
									<span class="flex items-center gap-1 text-green-600">
										<Lock class="h-3 w-3" />
										Enabled
									</span>
								{:else}
									<span class="flex items-center gap-1 text-gray-400">
										<LockOpen class="h-3 w-3" />
										Disabled
									</span>
								{/if}
							</dd>
						</div>
					</dl>
				</ConfigCard>

				<!-- Listener TLS Configuration -->
				{#if listenerTls}
					<ConfigCard title="Listener TLS" variant="blue">
						<dl class="grid grid-cols-2 gap-4 text-sm">
							<div>
								<dt class="text-gray-500">Certificate</dt>
								<dd class="text-gray-900">{listenerTls.hasCertificate ? 'Configured' : 'Not configured'}</dd>
							</div>
							<div>
								<dt class="text-gray-500">Client Cert Required</dt>
								<dd class="text-gray-900">{listenerTls.requireClientCert ? 'Yes (mTLS)' : 'No'}</dd>
							</div>
							{#if listenerTls.minTlsVersion}
								<div>
									<dt class="text-gray-500">Min TLS Version</dt>
									<dd class="text-gray-900">{listenerTls.minTlsVersion}</dd>
								</div>
							{/if}
						</dl>
					</ConfigCard>
				{/if}

				<!-- HTTP Filters -->
				{#if httpFilters.length > 0}
					<ConfigCard title="HTTP Filters" variant="blue">
						<div class="flex flex-wrap gap-2">
							{#each httpFilters as filter}
								<span class="px-2 py-1 text-xs font-medium bg-blue-100 text-blue-800 rounded">
									{formatHttpFilterName(filter)}
								</span>
							{/each}
						</div>
					</ConfigCard>
				{/if}

				<!-- Filter Chains (collapsible if multiple) -->
				{#if filterChains.length > 1}
					<ConfigCard title="Filter Chains" variant="blue" collapsible defaultCollapsed>
						<div class="space-y-3">
							{#each filterChains as fc, i}
								<div class="p-3 bg-white rounded border border-blue-200">
									<div class="font-medium text-gray-900">{fc.name}</div>
									{#if fc.filters.length > 0}
										<div class="mt-2 space-y-1">
											{#each fc.filters as filter}
												<div class="text-sm text-gray-600">
													{filter}
													{#if fc.routeConfigName}
														<span class="text-gray-400"> &rarr; {fc.routeConfigName}</span>
													{/if}
												</div>
											{/each}
										</div>
									{/if}
									{#if fc.hasTls}
										<div class="mt-2 flex items-center gap-1 text-green-600 text-sm">
											<Lock class="h-3 w-3" />
											TLS Enabled
										</div>
									{/if}
								</div>
							{/each}
						</div>
					</ConfigCard>
				{/if}

				<!-- Tracing Configuration -->
				{#if tracingConfig}
					<ConfigCard title="Tracing" variant="blue">
						<dl class="grid grid-cols-2 gap-4 text-sm">
							<div>
								<dt class="text-gray-500">Provider</dt>
								<dd class="text-gray-900">{tracingConfig.provider}</dd>
							</div>
							{#if tracingConfig.samplingRate !== undefined}
								<div>
									<dt class="text-gray-500">Sampling Rate</dt>
									<dd class="text-gray-900">{tracingConfig.samplingRate}%</dd>
								</div>
							{/if}
						</dl>
					</ConfigCard>
				{/if}

				<!-- Access Logging -->
				{#if accessLogConfig}
					<ConfigCard title="Access Logging" variant="blue">
						<dl class="grid grid-cols-2 gap-4 text-sm">
							{#if accessLogConfig.path}
								<div>
									<dt class="text-gray-500">Path</dt>
									<dd class="font-mono text-gray-900">{accessLogConfig.path}</dd>
								</div>
							{/if}
							<div>
								<dt class="text-gray-500">Format</dt>
								<dd class="text-gray-900">{accessLogConfig.format}</dd>
							</div>
						</dl>
					</ConfigCard>
				{/if}
			{/if}

			<!-- ============================================ -->
			<!-- Enhanced Clusters Section -->
			<!-- ============================================ -->
			{#if routeClusters.length > 0}
				{#each routeClusters as cluster}
					{@const clusterConfig = cluster.config || {}}
					{@const endpoints = extractClusterEndpoints(cluster)}
					{@const lbPolicy = extractLoadBalancingPolicy(clusterConfig)}
					{@const healthChecks = extractHealthChecks(clusterConfig)}
					{@const circuitBreaker = extractCircuitBreaker(clusterConfig)}
					{@const outlierDetection = extractOutlierDetection(clusterConfig)}
					{@const clusterTls = extractClusterTls(clusterConfig)}

					<!-- Cluster Overview -->
					<ConfigCard title="Cluster: {cluster.serviceName}" variant="green">
						<dl class="grid grid-cols-2 gap-4 text-sm">
							<div>
								<dt class="text-gray-500">Cluster Name</dt>
								<dd class="font-mono text-gray-900">{cluster.name}</dd>
							</div>
							<div>
								<dt class="text-gray-500">LB Policy</dt>
								<dd class="text-gray-900">
									{formatLbPolicy(lbPolicy.policy)}
									{#if lbPolicy.params}
										<span class="text-gray-500 text-xs ml-1">({lbPolicy.params})</span>
									{/if}
								</dd>
							</div>
							<div>
								<dt class="text-gray-500">Connect Timeout</dt>
								<dd class="text-gray-900">{clusterConfig.connectTimeout || '5s'}</dd>
							</div>
							<div>
								<dt class="text-gray-500">TLS</dt>
								<dd class="text-gray-900">
									{#if clusterTls}
										<span class="flex items-center gap-1 text-green-600">
											<Lock class="h-3 w-3" />
											Enabled
										</span>
									{:else}
										<span class="flex items-center gap-1 text-gray-400">
											<LockOpen class="h-3 w-3" />
											Disabled
										</span>
									{/if}
								</dd>
							</div>
						</dl>
					</ConfigCard>

					<!-- Endpoints -->
					{#if endpoints.length > 0}
						<ConfigCard title="Endpoints" variant="green">
							<div class="space-y-2">
								{#each endpoints as ep}
									<div class="flex items-center justify-between p-2 bg-white rounded border border-green-200">
										<span class="font-mono text-gray-900">{ep.address}:{ep.port}</span>
										<span class="text-xs px-2 py-0.5 rounded bg-green-100 text-green-700">Active</span>
									</div>
								{/each}
							</div>
						</ConfigCard>
					{/if}

					<!-- Health Checks -->
					{#if healthChecks.length > 0}
						<ConfigCard title="Health Checks" variant="green">
							{#each healthChecks as hc}
								<dl class="grid grid-cols-2 gap-4 text-sm">
									<div>
										<dt class="text-gray-500">Type</dt>
										<dd class="text-gray-900">{hc.type}</dd>
									</div>
									{#if hc.path}
										<div>
											<dt class="text-gray-500">Path</dt>
											<dd class="font-mono text-gray-900">{hc.path}</dd>
										</div>
									{/if}
									<div>
										<dt class="text-gray-500">Interval</dt>
										<dd class="text-gray-900">{hc.interval}</dd>
									</div>
									<div>
										<dt class="text-gray-500">Timeout</dt>
										<dd class="text-gray-900">{hc.timeout}</dd>
									</div>
									{#if hc.healthyThreshold}
										<div>
											<dt class="text-gray-500">Healthy Threshold</dt>
											<dd class="text-gray-900">{hc.healthyThreshold}</dd>
										</div>
									{/if}
									{#if hc.unhealthyThreshold}
										<div>
											<dt class="text-gray-500">Unhealthy Threshold</dt>
											<dd class="text-gray-900">{hc.unhealthyThreshold}</dd>
										</div>
									{/if}
								</dl>
							{/each}
						</ConfigCard>
					{/if}

					<!-- Circuit Breaker -->
					{#if circuitBreaker}
						<ConfigCard title="Circuit Breaker" variant="yellow">
							<dl class="grid grid-cols-2 gap-4 text-sm">
								<div>
									<dt class="text-gray-500">Max Connections</dt>
									<dd class="text-gray-900">{circuitBreaker.maxConnections}</dd>
								</div>
								<div>
									<dt class="text-gray-500">Max Pending Requests</dt>
									<dd class="text-gray-900">{circuitBreaker.maxPendingRequests}</dd>
								</div>
								<div>
									<dt class="text-gray-500">Max Requests</dt>
									<dd class="text-gray-900">{circuitBreaker.maxRequests}</dd>
								</div>
								<div>
									<dt class="text-gray-500">Max Retries</dt>
									<dd class="text-gray-900">{circuitBreaker.maxRetries}</dd>
								</div>
							</dl>
						</ConfigCard>
					{/if}

					<!-- Outlier Detection -->
					{#if outlierDetection}
						<ConfigCard title="Outlier Detection" variant="orange">
							<dl class="grid grid-cols-2 gap-4 text-sm">
								<div>
									<dt class="text-gray-500">Consecutive 5xx</dt>
									<dd class="text-gray-900">{outlierDetection.consecutive5xx}</dd>
								</div>
								<div>
									<dt class="text-gray-500">Interval</dt>
									<dd class="text-gray-900">{outlierDetection.interval}</dd>
								</div>
								<div>
									<dt class="text-gray-500">Base Ejection Time</dt>
									<dd class="text-gray-900">{outlierDetection.baseEjectionTime}</dd>
								</div>
								<div>
									<dt class="text-gray-500">Max Ejection %</dt>
									<dd class="text-gray-900">{outlierDetection.maxEjectionPercent}%</dd>
								</div>
							</dl>
						</ConfigCard>
					{/if}

					<!-- Cluster TLS Configuration -->
					{#if clusterTls}
						<ConfigCard title="Upstream TLS" variant="green">
							<dl class="grid grid-cols-2 gap-4 text-sm">
								{#if clusterTls.serverName}
									<div>
										<dt class="text-gray-500">Server Name (SNI)</dt>
										<dd class="font-mono text-gray-900">{clusterTls.serverName}</dd>
									</div>
								{/if}
								<div>
									<dt class="text-gray-500">Verify Certificate</dt>
									<dd class="text-gray-900">{clusterTls.verifyCertificate ? 'Yes' : 'No'}</dd>
								</div>
								{#if clusterTls.minTlsVersion}
									<div>
										<dt class="text-gray-500">Min TLS Version</dt>
										<dd class="text-gray-900">{clusterTls.minTlsVersion}</dd>
									</div>
								{/if}
							</dl>
						</ConfigCard>
					{/if}
				{/each}
			{/if}

			<!-- Retry Policies -->
			{#if getRetryPoliciesForRoute(selectedRoute).length > 0}
				{@const retryPolicies = getRetryPoliciesForRoute(selectedRoute)}
				<ConfigCard title="Retry Policies" variant="orange">
					<div class="space-y-3">
						{#each retryPolicies as { routeName, policy }}
							<div class="p-3 bg-white rounded border border-orange-200">
								<div class="font-medium text-gray-900 text-sm mb-2">{routeName}</div>
								<dl class="grid grid-cols-2 gap-2 text-sm">
									<div>
										<dt class="text-gray-500">Max Retries</dt>
										<dd class="font-medium text-gray-900">{policy.numRetries}</dd>
									</div>
									<div>
										<dt class="text-gray-500">Per Try Timeout</dt>
										<dd class="font-medium text-gray-900">{policy.perTryTimeout || '-'}</dd>
									</div>
									<div class="col-span-2">
										<dt class="text-gray-500">Retry On</dt>
										<dd class="font-medium text-gray-900">{formatRetryOn(policy.retryOn)}</dd>
									</div>
									{#if policy.retryBackOff}
										<div class="col-span-2">
											<dt class="text-gray-500">Backoff</dt>
											<dd class="font-medium text-gray-900">{formatBackoff(policy)}</dd>
										</div>
									{/if}
								</dl>
							</div>
						{/each}
					</div>
				</ConfigCard>
			{/if}

			<!-- Domains -->
			{#if selectedRoute.config?.virtualHosts}
				<ConfigCard title="Domains" variant="gray" collapsible defaultCollapsed>
					<div class="space-y-4">
						{#each selectedRoute.config.virtualHosts as vh}
							<div class="p-3 bg-white rounded border border-gray-200">
								<div class="font-medium text-gray-900">{vh.name}</div>
								<div class="text-sm text-gray-500 mt-1">
									{vh.domains?.join(', ') || '*'}
								</div>
								{#if vh.routes}
									<div class="mt-2 text-sm text-gray-600">
										{vh.routes.length} route{vh.routes.length !== 1 ? 's' : ''}
									</div>
								{/if}
							</div>
						{/each}
					</div>
				</ConfigCard>
			{/if}
		</div>
	{/if}

	{#snippet footer()}
		<div class="flex justify-end gap-3">
			<Button variant="ghost" onclick={closeDrawer}>Close</Button>
			<Button variant="danger" onclick={() => selectedRoute && handleDelete(selectedRoute.name)}>
				Delete
			</Button>
		</div>
	{/snippet}
</DetailDrawer>
