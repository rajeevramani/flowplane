<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import { Eye, Pencil } from 'lucide-svelte';
	import DataTable from '$lib/components/DataTable.svelte';
	import FeatureBadges from '$lib/components/FeatureBadges.svelte';
	import StatusIndicator from '$lib/components/StatusIndicator.svelte';
	import DetailDrawer from '$lib/components/DetailDrawer.svelte';
	import ConfigCard from '$lib/components/ConfigCard.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import ClusterConfigEditor from '$lib/components/ClusterConfigEditor.svelte';
	import type {
		ClusterResponse,
		RouteResponse,
		ImportSummary,
		HealthCheckRequest,
		CircuitBreakersRequest,
		OutlierDetectionRequest,
		CreateClusterBody
	} from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import type { Unsubscriber } from 'svelte/store';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');
	let unsubscribe: Unsubscriber;

	// Data
	let clusters = $state<ClusterResponse[]>([]);
	let routes = $state<RouteResponse[]>([]);
	let imports = $state<ImportSummary[]>([]);

	// Drawer state
	let drawerOpen = $state(false);
	let selectedCluster = $state<ClusterResponse | null>(null);

	// Edit mode state
	let editMode = $state(false);
	let isSaving = $state(false);
	let saveError = $state<string | null>(null);
	let editedHealthChecks = $state<HealthCheckRequest[]>([]);
	let editedCircuitBreakers = $state<CircuitBreakersRequest | null>(null);
	let editedOutlierDetection = $state<OutlierDetectionRequest | null>(null);

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
			const [clustersData, routesData, importsData] = await Promise.all([
				apiClient.listClusters(),
				apiClient.listRoutes(),
				currentTeam ? apiClient.listImports(currentTeam) : Promise.resolve([])
			]);

			clusters = clustersData;
			routes = routesData;
			imports = importsData;
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Table columns
	const columns = [
		{ key: 'serviceName', label: 'Service', sortable: true },
		{ key: 'team', label: 'Team', sortable: true },
		{ key: 'endpoints', label: 'Endpoints' },
		{ key: 'features', label: 'Features' },
		{ key: 'lbPolicy', label: 'LB Policy' },
		{ key: 'status', label: 'Status' },
		{ key: 'source', label: 'Source' }
	];

	// Transform clusters for table display
	let tableData = $derived(
		clusters
			.filter((cluster) => {
				// Filter by team if a team is selected
				if (currentTeam && cluster.team !== currentTeam) return false;

				// Filter by search query
				if (!searchQuery) return true;
				const query = searchQuery.toLowerCase();
				return (
					cluster.name.toLowerCase().includes(query) ||
					cluster.serviceName.toLowerCase().includes(query) ||
					cluster.team.toLowerCase().includes(query)
				);
			})
			.map((cluster) => {
				const config = cluster.config || {};
				// Use extractEndpoints to handle both simple format and xDS format
				const endpointCount = extractEndpoints(config).length;
				const importRecord = cluster.importId
					? imports.find((i) => i.id === cluster.importId)
					: null;

				return {
					id: cluster.name,
					name: cluster.name,
					serviceName: cluster.serviceName,
					team: cluster.team,
					endpoints: endpointCount,
					lbPolicy: config.lbPolicy || 'ROUND_ROBIN',
					hasRetry: false,
					hasCircuitBreaker: Boolean(config.circuitBreakers),
					hasOutlierDetection: Boolean(config.outlierDetection),
					hasHealthCheck: Boolean(config.healthChecks?.length),
					source: importRecord ? importRecord.specName : 'Manual',
					sourceType: importRecord ? 'import' : 'manual',
					_raw: cluster
				};
			})
	);

	function openDrawer(row: Record<string, unknown>) {
		selectedCluster = (row._raw as ClusterResponse) || null;
		drawerOpen = true;
	}

	function closeDrawer() {
		drawerOpen = false;
		selectedCluster = null;
	}

	async function handleDelete(clusterName: string) {
		if (!confirm(`Are you sure you want to delete the cluster "${clusterName}"?`)) return;

		try {
			await apiClient.deleteCluster(clusterName);
			await loadData();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to delete cluster';
		}
	}

	function startEdit() {
		if (!selectedCluster) return;

		const config = selectedCluster.config || {};

		// Parse existing health checks - handle both flat format and xDS nested format
		editedHealthChecks = (config.healthChecks || []).map((hc: Record<string, unknown>) => {
			// Check if it's xDS nested format (has httpHealthCheck/tcpHealthCheck)
			const httpCheck = hc.httpHealthCheck as Record<string, unknown> | undefined;
			const tcpCheck = hc.tcpHealthCheck as Record<string, unknown> | undefined;

			if (httpCheck || tcpCheck) {
				// xDS nested format
				const type = httpCheck ? 'http' : 'tcp';
				return {
					type,
					path: httpCheck?.path as string | undefined,
					host: httpCheck?.host as string | undefined,
					method: httpCheck?.method as string | undefined,
					intervalSeconds: parseTimeoutSeconds(hc.interval as string | undefined),
					timeoutSeconds: parseTimeoutSeconds(hc.timeout as string | undefined),
					healthyThreshold: hc.healthyThreshold as number | undefined,
					unhealthyThreshold: hc.unhealthyThreshold as number | undefined,
					expectedStatuses: httpCheck?.expectedStatuses as number[] | undefined
				};
			} else {
				// Flat format from API (supports both camelCase and snake_case)
				return {
					type: (hc.type as string) || 'http',
					path: hc.path as string | undefined,
					host: hc.host as string | undefined,
					method: hc.method as string | undefined,
					intervalSeconds: (hc.intervalSeconds ?? hc.interval_seconds) as number | undefined,
					timeoutSeconds: (hc.timeoutSeconds ?? hc.timeout_seconds) as number | undefined,
					healthyThreshold: (hc.healthyThreshold ?? hc.healthy_threshold) as number | undefined,
					unhealthyThreshold: (hc.unhealthyThreshold ?? hc.unhealthy_threshold) as number | undefined,
					expectedStatuses: (hc.expectedStatuses ?? hc.expected_statuses) as number[] | undefined
				};
			}
		});

		// Parse existing circuit breakers - handle both xDS format (thresholds array) and flat format
		if (config.circuitBreakers) {
			const cb = config.circuitBreakers as Record<string, unknown>;

			if (cb.thresholds) {
				// xDS format with thresholds array
				const thresholds = cb.thresholds as { priority?: string; maxConnections?: number; maxPendingRequests?: number; maxRequests?: number; maxRetries?: number }[];
				const defaultThreshold = thresholds.find((t) => !t.priority || t.priority === 'DEFAULT');
				const highThreshold = thresholds.find((t) => t.priority === 'HIGH');

				editedCircuitBreakers = {
					default: defaultThreshold ? {
						maxConnections: defaultThreshold.maxConnections,
						maxPendingRequests: defaultThreshold.maxPendingRequests,
						maxRequests: defaultThreshold.maxRequests,
						maxRetries: defaultThreshold.maxRetries
					} : undefined,
					high: highThreshold ? {
						maxConnections: highThreshold.maxConnections,
						maxPendingRequests: highThreshold.maxPendingRequests,
						maxRequests: highThreshold.maxRequests,
						maxRetries: highThreshold.maxRetries
					} : undefined
				};
			} else if (cb.default || cb.high) {
				// Flat format from API (supports both camelCase and snake_case)
				const parseThreshold = (t: Record<string, unknown> | undefined) => t ? {
					maxConnections: (t.maxConnections ?? t.max_connections) as number | undefined,
					maxPendingRequests: (t.maxPendingRequests ?? t.max_pending_requests) as number | undefined,
					maxRequests: (t.maxRequests ?? t.max_requests) as number | undefined,
					maxRetries: (t.maxRetries ?? t.max_retries) as number | undefined
				} : undefined;

				editedCircuitBreakers = {
					default: parseThreshold(cb.default as Record<string, unknown> | undefined),
					high: parseThreshold(cb.high as Record<string, unknown> | undefined)
				};
			} else {
				editedCircuitBreakers = null;
			}
		} else {
			editedCircuitBreakers = null;
		}

		// Parse existing outlier detection - handle both xDS format and flat format
		if (config.outlierDetection) {
			const od = config.outlierDetection as Record<string, unknown>;
			editedOutlierDetection = {
				consecutive5xx: (od.consecutive5xx ?? od.consecutive_5xx) as number | undefined,
				intervalSeconds: (od.intervalSeconds ?? od.interval_seconds ?? parseTimeoutSeconds(od.interval as string | undefined)) as number | undefined,
				baseEjectionTimeSeconds: (od.baseEjectionTimeSeconds ?? od.base_ejection_time_seconds ?? parseTimeoutSeconds(od.baseEjectionTime as string | undefined)) as number | undefined,
				maxEjectionPercent: (od.maxEjectionPercent ?? od.max_ejection_percent) as number | undefined,
				minHosts: (od.minHosts ?? od.min_hosts ?? od.successRateMinimumHosts) as number | undefined
			};
		} else {
			editedOutlierDetection = null;
		}

		editMode = true;
		saveError = null;
	}

	function parseTimeoutSeconds(value: string | undefined): number | undefined {
		if (!value) return undefined;
		// Parse "10s" or "5s" format
		const match = value.match(/^(\d+)s?$/);
		return match ? parseInt(match[1], 10) : undefined;
	}

	function cancelEdit() {
		editMode = false;
		saveError = null;
	}

	function extractEndpoints(config: Record<string, unknown>): { host: string; port: number }[] {
		// Simple format: endpoints directly on config
		if (Array.isArray(config.endpoints)) {
			const endpoints = config.endpoints as { host?: string; port?: number }[];
			return endpoints
				.filter(ep => ep.host && ep.host !== '')
				.map(ep => ({
					host: ep.host as string,
					port: ep.port || 8080
				}));
		}

		// xDS format: nested in loadAssignment
		const loadAssignment = (config.loadAssignment || config.load_assignment) as Record<string, unknown> | undefined;
		if (!loadAssignment) return [];

		const localityEndpoints = (loadAssignment.endpoints || loadAssignment.locality_lb_endpoints) as Record<string, unknown>[] | undefined;
		if (!localityEndpoints || localityEndpoints.length === 0) return [];

		const lbEndpoints = (localityEndpoints[0].lbEndpoints || localityEndpoints[0].lb_endpoints) as Record<string, unknown>[] | undefined;
		if (!lbEndpoints) return [];

		return lbEndpoints.map((ep) => {
			const endpoint = ep.endpoint as Record<string, unknown> | undefined;
			const address = endpoint?.address as Record<string, unknown> | undefined;
			const socketAddress = (address?.socketAddress || address?.socket_address) as Record<string, unknown> | undefined;

			return {
				host: (socketAddress?.address || '') as string,
				port: (socketAddress?.portValue || socketAddress?.port_value || 8080) as number
			};
		}).filter(ep => ep.host !== '');
	}

	async function saveEdit() {
		if (!selectedCluster) return;

		isSaving = true;
		saveError = null;

		try {
			const config = selectedCluster.config || {};
			const endpoints = extractEndpoints(config);

			if (endpoints.length === 0) {
				saveError = 'Unable to extract endpoints from cluster configuration. Please delete and recreate the cluster.';
				isSaving = false;
				return;
			}

			const body: CreateClusterBody = {
				team: selectedCluster.team,
				name: selectedCluster.name,
				serviceName: selectedCluster.serviceName,
				endpoints,
				lbPolicy: (config.lbPolicy || config.lb_policy) as CreateClusterBody['lbPolicy'],
				healthChecks: editedHealthChecks.length > 0 ? editedHealthChecks : undefined,
				circuitBreakers: editedCircuitBreakers ?? undefined,
				outlierDetection: editedOutlierDetection ?? undefined
			};

			console.log('Saving cluster with body:', JSON.stringify(body, null, 2));
			const response = await apiClient.updateCluster(selectedCluster.name, body);
			console.log('Update response:', JSON.stringify(response, null, 2));
			await loadData();

			// Update selected cluster with new data
			const updatedCluster = clusters.find((c) => c.name === selectedCluster.name);
			if (updatedCluster) {
				selectedCluster = updatedCluster;
			}

			editMode = false;
		} catch (err) {
			saveError = err instanceof Error ? err.message : 'Failed to save changes';
		} finally {
			isSaving = false;
		}
	}

	function handleHealthChecksChange(checks: HealthCheckRequest[]) {
		editedHealthChecks = checks;
	}

	function handleCircuitBreakersChange(cb: CircuitBreakersRequest | null) {
		editedCircuitBreakers = cb;
	}

	function handleOutlierDetectionChange(od: OutlierDetectionRequest | null) {
		editedOutlierDetection = od;
	}

	// Get routes using this cluster
	function getRoutesForCluster(cluster: ClusterResponse): RouteResponse[] {
		return routes.filter((route) => {
			return route.config?.virtualHosts?.some((vh: { routes?: { route?: { cluster?: string } }[] }) =>
				vh.routes?.some((r) => r.route?.cluster === cluster.name)
			);
		});
	}

	// Format LB policy for display
	function formatLbPolicy(policy: string): string {
		return policy.replace(/_/g, ' ').toLowerCase().replace(/\b\w/g, (c) => c.toUpperCase());
	}
</script>

<!-- Page Header -->
<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="text-2xl font-bold text-gray-900">Clusters</h1>
		<p class="mt-1 text-sm text-gray-600">Backend services and upstream endpoints</p>
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
		placeholder="Search clusters..."
		class="w-full max-w-md px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
	/>
</div>

<!-- Data Table -->
<DataTable
	{columns}
	data={tableData}
	loading={isLoading}
	emptyMessage="No clusters found."
	rowKey="id"
	onRowClick={openDrawer}
>
	{#snippet cell({ row, column })}
		{#if column.key === 'serviceName'}
			<span class="font-medium text-blue-600 hover:text-blue-800">{row.serviceName}</span>
		{:else if column.key === 'team'}
			<Badge variant="indigo">{row.team}</Badge>
		{:else if column.key === 'endpoints'}
			<span class="text-gray-600">{row.endpoints} endpoint{row.endpoints !== 1 ? 's' : ''}</span>
		{:else if column.key === 'features'}
			<FeatureBadges
				hasRetry={row.hasRetry as boolean}
				hasCircuitBreaker={row.hasCircuitBreaker as boolean}
				hasOutlierDetection={row.hasOutlierDetection as boolean}
				hasHealthCheck={row.hasHealthCheck as boolean}
			/>
		{:else if column.key === 'lbPolicy'}
			<span class="text-gray-600">{formatLbPolicy(row.lbPolicy as string)}</span>
		{:else if column.key === 'status'}
			<StatusIndicator status="active" />
		{:else if column.key === 'source'}
			{#if row.sourceType === 'import'}
				<Badge variant="purple">{row.source}</Badge>
			{:else}
				<span class="text-gray-500">{row.source}</span>
			{/if}
		{:else}
			{String(row[column.key] ?? '')}
		{/if}
	{/snippet}

	{#snippet actions({ row })}
		<button
			onclick={(e) => {
				e.stopPropagation();
				openDrawer(row);
			}}
			class="p-1.5 rounded hover:bg-gray-100 text-gray-500 hover:text-blue-600 transition-colors"
			title="View details"
		>
			<Eye class="h-4 w-4" />
		</button>
	{/snippet}
</DataTable>

<!-- Detail Drawer -->
<DetailDrawer
	open={drawerOpen}
	title={selectedCluster?.serviceName || ''}
	subtitle={selectedCluster ? `Team: ${selectedCluster.team}${editMode ? ' - Editing' : ''}` : undefined}
	onClose={() => { editMode = false; closeDrawer(); }}
>
	{#if selectedCluster}
		{@const config = selectedCluster.config || {}}
		<div class="space-y-6">
			{#if editMode}
				<!-- Edit Mode -->
				{#if saveError}
					<div class="bg-red-50 border-l-4 border-red-500 rounded-md p-3">
						<p class="text-red-800 text-sm">{saveError}</p>
					</div>
				{/if}

				<div class="p-4 bg-blue-50 rounded-lg border border-blue-200">
					<h3 class="text-sm font-medium text-blue-900 mb-3">Edit Resilience Configuration</h3>
					<ClusterConfigEditor
						healthChecks={editedHealthChecks}
						circuitBreakers={editedCircuitBreakers}
						outlierDetection={editedOutlierDetection}
						onHealthChecksChange={handleHealthChecksChange}
						onCircuitBreakersChange={handleCircuitBreakersChange}
						onOutlierDetectionChange={handleOutlierDetectionChange}
					/>
				</div>
			{:else}
				<!-- View Mode -->
				<!-- Overview -->
				<ConfigCard title="Overview" variant="gray">
					<dl class="grid grid-cols-2 gap-4 text-sm">
						<div>
							<dt class="text-gray-500">Cluster Name</dt>
							<dd class="font-mono text-gray-900">{selectedCluster.name}</dd>
						</div>
						<div>
							<dt class="text-gray-500">LB Policy</dt>
							<dd class="text-gray-900">{formatLbPolicy(config.lbPolicy || 'ROUND_ROBIN')}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Connect Timeout</dt>
							<dd class="text-gray-900">{config.connectTimeout || '5s'}</dd>
						</div>
						<div>
							<dt class="text-gray-500">DNS Lookup Family</dt>
							<dd class="text-gray-900">{config.dnsLookupFamily || 'AUTO'}</dd>
						</div>
					</dl>
				</ConfigCard>

				<!-- Endpoints -->
				{#if (config.loadAssignment?.endpoints?.[0]?.lbEndpoints || []).length > 0}
					{@const endpoints = config.loadAssignment?.endpoints?.[0]?.lbEndpoints || []}
					<ConfigCard title="Endpoints" variant="green">
						<div class="space-y-2">
							{#each endpoints as ep}
								{@const addr = ep.endpoint?.address?.socketAddress}
								{#if addr}
									<div class="flex items-center justify-between p-2 bg-white rounded border border-green-200">
										<span class="font-mono text-gray-900">{addr.address}:{addr.portValue}</span>
										<StatusIndicator status="active" showLabel={false} />
									</div>
								{/if}
							{/each}
						</div>
					</ConfigCard>
				{/if}

				<!-- Circuit Breaker -->
				{#if config.circuitBreakers}
					{@const cb = config.circuitBreakers}
					{@const thresholds = cb.thresholds?.[0] || cb.default || {}}
					{@const maxConn = thresholds.maxConnections ?? thresholds.max_connections ?? 1024}
					{@const maxPending = thresholds.maxPendingRequests ?? thresholds.max_pending_requests ?? 1024}
					{@const maxReq = thresholds.maxRequests ?? thresholds.max_requests ?? 1024}
					{@const maxRetries = thresholds.maxRetries ?? thresholds.max_retries ?? 3}
					<ConfigCard title="Circuit Breaker" variant="yellow">
						<dl class="grid grid-cols-2 gap-4 text-sm">
							<div>
								<dt class="text-gray-500">Max Connections</dt>
								<dd class="text-gray-900">{maxConn}</dd>
							</div>
							<div>
								<dt class="text-gray-500">Max Pending Requests</dt>
								<dd class="text-gray-900">{maxPending}</dd>
							</div>
							<div>
								<dt class="text-gray-500">Max Requests</dt>
								<dd class="text-gray-900">{maxReq}</dd>
							</div>
							<div>
								<dt class="text-gray-500">Max Retries</dt>
								<dd class="text-gray-900">{maxRetries}</dd>
							</div>
						</dl>
					</ConfigCard>
				{/if}

				<!-- Outlier Detection -->
				{#if config.outlierDetection}
					{@const od = config.outlierDetection}
					{@const odConsecutive = od.consecutive5xx ?? od.consecutive_5xx ?? 5}
					{@const odInterval = od.interval_seconds ?? od.intervalSeconds ?? od.interval ?? 10}
					{@const odEjectionTime = od.base_ejection_time_seconds ?? od.baseEjectionTimeSeconds ?? od.baseEjectionTime ?? 30}
					{@const odMaxPercent = od.max_ejection_percent ?? od.maxEjectionPercent ?? 10}
					<ConfigCard title="Outlier Detection" variant="orange">
						<dl class="grid grid-cols-2 gap-4 text-sm">
							<div>
								<dt class="text-gray-500">Consecutive 5xx</dt>
								<dd class="text-gray-900">{odConsecutive}</dd>
							</div>
							<div>
								<dt class="text-gray-500">Interval</dt>
								<dd class="text-gray-900">{typeof odInterval === 'number' ? `${odInterval}s` : odInterval}</dd>
							</div>
							<div>
								<dt class="text-gray-500">Base Ejection Time</dt>
								<dd class="text-gray-900">{typeof odEjectionTime === 'number' ? `${odEjectionTime}s` : odEjectionTime}</dd>
							</div>
							<div>
								<dt class="text-gray-500">Max Ejection %</dt>
								<dd class="text-gray-900">{odMaxPercent}%</dd>
							</div>
						</dl>
					</ConfigCard>
				{/if}

				<!-- Health Checks -->
				{#if config.healthChecks?.length}
					<ConfigCard title="Health Checks" variant="green">
						{#each config.healthChecks as hc}
							{@const hcType = hc.type?.toUpperCase() || (hc.httpHealthCheck ? 'HTTP' : 'TCP')}
							{@const hcPath = hc.path || hc.httpHealthCheck?.path}
							{@const hcInterval = hc.interval_seconds ?? hc.intervalSeconds ?? hc.interval ?? 5}
							{@const hcTimeout = hc.timeout_seconds ?? hc.timeoutSeconds ?? hc.timeout ?? 5}
							<dl class="grid grid-cols-2 gap-4 text-sm">
								<div>
									<dt class="text-gray-500">Type</dt>
									<dd class="text-gray-900">{hcType}</dd>
								</div>
								{#if hcPath}
									<div>
										<dt class="text-gray-500">Path</dt>
										<dd class="font-mono text-gray-900">{hcPath}</dd>
									</div>
								{/if}
								<div>
									<dt class="text-gray-500">Interval</dt>
									<dd class="text-gray-900">{typeof hcInterval === 'number' ? `${hcInterval}s` : hcInterval}</dd>
								</div>
								<div>
									<dt class="text-gray-500">Timeout</dt>
									<dd class="text-gray-900">{typeof hcTimeout === 'number' ? `${hcTimeout}s` : hcTimeout}</dd>
								</div>
							</dl>
						{/each}
					</ConfigCard>
				{/if}

				<!-- Associated Routes -->
				{#if getRoutesForCluster(selectedCluster).length > 0}
					{@const clusterRoutes = getRoutesForCluster(selectedCluster)}
					<ConfigCard title="Associated Routes" variant="blue" collapsible defaultCollapsed>
						<div class="space-y-2">
							{#each clusterRoutes as route}
								<div class="p-2 bg-white rounded border border-blue-200">
									<div class="font-medium text-gray-900">{route.name}</div>
									<div class="text-sm text-gray-500">{route.pathPrefix}</div>
								</div>
							{/each}
						</div>
					</ConfigCard>
				{/if}
			{/if}
		</div>
	{/if}

	{#snippet footer()}
		<div class="flex justify-end gap-3">
			{#if editMode}
				<Button variant="ghost" onclick={cancelEdit} disabled={isSaving}>Cancel</Button>
				<Button onclick={saveEdit} disabled={isSaving}>
					{isSaving ? 'Saving...' : 'Save Changes'}
				</Button>
			{:else}
				<Button variant="ghost" onclick={closeDrawer}>Close</Button>
				<Button variant="ghost" onclick={startEdit}>
					<Pencil class="h-4 w-4 mr-1" />
					Edit
				</Button>
				<Button variant="danger" onclick={() => selectedCluster && handleDelete(selectedCluster.name)}>
					Delete
				</Button>
			{/if}
		</div>
	{/snippet}
</DetailDrawer>
