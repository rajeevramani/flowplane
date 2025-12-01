<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { Plus, Trash2, ChevronDown, ChevronUp, ArrowLeft } from 'lucide-svelte';
	import { selectedTeam } from '$lib/stores/team';
	import type {
		ClusterResponse,
		HealthCheckRequest,
		CircuitBreakersRequest,
		OutlierDetectionRequest
	} from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import JsonPanel from '$lib/components/route-config/JsonPanel.svelte';
	import ClusterConfigEditor from '$lib/components/ClusterConfigEditor.svelte';

	interface EndpointFormState {
		id: string;
		host: string;
		port: number;
	}

	interface FormState {
		name: string;
		team: string;
		serviceName: string;
		endpoints: EndpointFormState[];
		lbPolicy: string;
		healthChecks: HealthCheckRequest[];
		circuitBreakers: CircuitBreakersRequest | null;
		outlierDetection: OutlierDetectionRequest | null;
		connectTimeout?: number;
		dnsLookupFamily?: string;
	}

	let currentTeam = $state<string>('');
	let clusterName = $derived($page.params.name);
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let isSubmitting = $state(false);
	let activeTab = $state<'configuration' | 'json'>('configuration');
	let resilienceExpanded = $state(false);
	let advancedExpanded = $state(false);
	let originalCluster = $state<ClusterResponse | null>(null);

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Initialize empty form state
	let formState = $state<FormState>({
		name: '',
		team: currentTeam,
		serviceName: '',
		endpoints: [],
		lbPolicy: 'ROUND_ROBIN',
		healthChecks: [],
		circuitBreakers: null,
		outlierDetection: null
	});

	// Load cluster data
	onMount(async () => {
		await loadData();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		if (!clusterName) {
			error = 'Cluster name is required';
			isLoading = false;
			return;
		}

		try {
			const cluster = await apiClient.getCluster(clusterName);
			originalCluster = cluster;

			// Parse cluster into form state
			formState = parseClusterToForm(cluster);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load cluster';
		} finally {
			isLoading = false;
		}
	}

	// Extract endpoints from cluster config (handles both simple and xDS format)
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

	function parseTimeoutSeconds(value: string | undefined): number | undefined {
		if (!value) return undefined;
		// Parse "10s" or "5s" format
		const match = value.match(/^(\d+)s?$/);
		return match ? parseInt(match[1], 10) : undefined;
	}

	// Parse ClusterResponse to form state
	function parseClusterToForm(cluster: ClusterResponse): FormState {
		const config = cluster.config || {};
		const endpoints = extractEndpoints(config);

		// Parse health checks
		const healthChecks = (config.healthChecks || []).map((hc: Record<string, unknown>) => {
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
				// Flat format from API
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

		// Parse circuit breakers
		let circuitBreakers: CircuitBreakersRequest | null = null;
		if (config.circuitBreakers) {
			const cb = config.circuitBreakers as Record<string, unknown>;

			if (cb.thresholds) {
				// xDS format with thresholds array
				const thresholds = cb.thresholds as { priority?: string; maxConnections?: number; maxPendingRequests?: number; maxRequests?: number; maxRetries?: number }[];
				const defaultThreshold = thresholds.find((t) => !t.priority || t.priority === 'DEFAULT');
				const highThreshold = thresholds.find((t) => t.priority === 'HIGH');

				circuitBreakers = {
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
				// Flat format from API
				const parseThreshold = (t: Record<string, unknown> | undefined) => t ? {
					maxConnections: (t.maxConnections ?? t.max_connections) as number | undefined,
					maxPendingRequests: (t.maxPendingRequests ?? t.max_pending_requests) as number | undefined,
					maxRequests: (t.maxRequests ?? t.max_requests) as number | undefined,
					maxRetries: (t.maxRetries ?? t.max_retries) as number | undefined
				} : undefined;

				circuitBreakers = {
					default: parseThreshold(cb.default as Record<string, unknown> | undefined),
					high: parseThreshold(cb.high as Record<string, unknown> | undefined)
				};
			}
		}

		// Parse outlier detection
		let outlierDetection: OutlierDetectionRequest | null = null;
		if (config.outlierDetection) {
			const od = config.outlierDetection as Record<string, unknown>;
			outlierDetection = {
				consecutive5xx: (od.consecutive5xx ?? od.consecutive_5xx) as number | undefined,
				intervalSeconds: (od.intervalSeconds ?? od.interval_seconds ?? parseTimeoutSeconds(od.interval as string | undefined)) as number | undefined,
				baseEjectionTimeSeconds: (od.baseEjectionTimeSeconds ?? od.base_ejection_time_seconds ?? parseTimeoutSeconds(od.baseEjectionTime as string | undefined)) as number | undefined,
				maxEjectionPercent: (od.maxEjectionPercent ?? od.max_ejection_percent) as number | undefined,
				minHosts: (od.minHosts ?? od.min_hosts ?? od.successRateMinimumHosts) as number | undefined
			};
		}

		// Parse connect timeout
		const connectTimeout = parseTimeoutSeconds(config.connectTimeout as string | undefined);

		return {
			name: cluster.name,
			team: cluster.team,
			serviceName: cluster.serviceName,
			endpoints: endpoints.map((ep, i) => ({
				id: `ep-${i}-${Date.now()}`,
				host: ep.host,
				port: ep.port
			})),
			lbPolicy: (config.lbPolicy || config.lb_policy || 'ROUND_ROBIN') as string,
			healthChecks,
			circuitBreakers,
			outlierDetection,
			connectTimeout,
			dnsLookupFamily: config.dnsLookupFamily as string | undefined
		};
	}

	// Build JSON payload from form state
	let jsonPayload = $derived(buildClusterJSON(formState));

	function buildClusterJSON(form: FormState): string {
		const payload: any = {
			team: form.team || currentTeam,
			name: form.name || '',
			serviceName: form.serviceName || '',
			endpoints: form.endpoints
				.filter(ep => ep.host && ep.host.trim() !== '')
				.map(ep => ({
					host: ep.host,
					port: ep.port
				})),
			lbPolicy: form.lbPolicy
		};

		// Add optional fields
		if (form.healthChecks.length > 0) {
			payload.healthChecks = form.healthChecks;
		}

		if (form.circuitBreakers) {
			payload.circuitBreakers = form.circuitBreakers;
		}

		if (form.outlierDetection) {
			payload.outlierDetection = form.outlierDetection;
		}

		if (form.connectTimeout) {
			payload.connectTimeout = form.connectTimeout;
		}

		if (form.dnsLookupFamily) {
			payload.dnsLookupFamily = form.dnsLookupFamily;
		}

		return JSON.stringify(payload, null, 2);
	}

	// Add endpoint
	function handleAddEndpoint() {
		formState.endpoints = [
			...formState.endpoints,
			{
				id: `ep-${Date.now()}`,
				host: '',
				port: 8080
			}
		];
	}

	// Remove endpoint
	function handleRemoveEndpoint(index: number) {
		formState.endpoints = formState.endpoints.filter((_, i) => i !== index);
	}

	// Validate form
	function validateForm(): string | null {
		if (!formState.name) return 'Cluster name is required';
		if (!formState.serviceName) return 'Service name is required';

		const validEndpoints = formState.endpoints.filter(ep => ep.host && ep.host.trim() !== '');
		if (validEndpoints.length === 0) return 'At least one endpoint is required';

		// Validate endpoint format
		for (const ep of validEndpoints) {
			if (ep.port < 1 || ep.port > 65535) {
				return `Invalid port ${ep.port} for endpoint ${ep.host}`;
			}
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
			console.log('Updating cluster:', payload);
			await apiClient.updateCluster(clusterName!, payload);
			goto('/clusters');
		} catch (e) {
			console.error('Update failed:', e);
			if (e && typeof e === 'object' && 'message' in e) {
				error = (e as any).message;
			} else {
				error = 'Failed to update cluster';
			}
		} finally {
			isSubmitting = false;
		}
	}

	// Handle cancel
	function handleCancel() {
		goto('/clusters');
	}

	// Handle resilience config changes
	function handleHealthChecksChange(checks: HealthCheckRequest[]) {
		formState.healthChecks = checks;
	}

	function handleCircuitBreakersChange(cb: CircuitBreakersRequest | null) {
		formState.circuitBreakers = cb;
	}

	function handleOutlierDetectionChange(od: OutlierDetectionRequest | null) {
		formState.outlierDetection = od;
	}
</script>

{#if isLoading}
	<div class="min-h-screen bg-gray-100 flex items-center justify-center">
		<div class="flex flex-col items-center gap-3">
			<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
			<span class="text-sm text-gray-600">Loading cluster...</span>
		</div>
	</div>
{:else if error && !originalCluster}
	<div class="min-h-screen bg-gray-100 flex items-center justify-center">
		<div class="bg-white rounded-lg shadow-sm border border-red-200 p-8 max-w-md">
			<h2 class="text-xl font-bold text-red-900 mb-2">Error Loading Cluster</h2>
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
							<h1 class="text-3xl font-bold text-gray-900">Edit Cluster</h1>
							<p class="text-sm text-gray-600 mt-1">
								Modify the cluster configuration for <span class="font-medium">{formState.name}</span>
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
						<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
							<div>
								<label class="block text-sm font-medium text-gray-700 mb-1">
									Cluster Name <span class="text-red-500">*</span>
								</label>
								<input
									type="text"
									bind:value={formState.name}
									disabled
									class="w-full px-3 py-2 border border-gray-300 rounded-md bg-gray-100 text-gray-600 cursor-not-allowed"
								/>
								<p class="text-xs text-gray-500 mt-1">
									Cluster name cannot be changed after creation
								</p>
							</div>

							<div>
								<label class="block text-sm font-medium text-gray-700 mb-1">
									Service Name <span class="text-red-500">*</span>
								</label>
								<input
									type="text"
									bind:value={formState.serviceName}
									placeholder="e.g., user-service"
									class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<p class="text-xs text-gray-500 mt-1">
									Descriptive name for the service
								</p>
							</div>
						</div>
					</div>
				</div>

				<!-- Endpoints -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
					<div class="flex items-center justify-between mb-4">
						<div>
							<h2 class="text-lg font-semibold text-gray-900">Endpoints</h2>
							<p class="text-sm text-gray-600">
								Backend service endpoints for this cluster
							</p>
						</div>
						<button
							onclick={handleAddEndpoint}
							class="px-3 py-1.5 text-sm text-blue-600 border border-blue-600 rounded-md hover:bg-blue-50 transition-colors"
						>
							<Plus class="h-4 w-4 inline mr-1" />
							Add Endpoint
						</button>
					</div>

					<div class="space-y-3">
						{#each formState.endpoints as endpoint, index}
							<div class="flex gap-3 items-start">
								<div class="flex-1">
									<input
										type="text"
										bind:value={endpoint.host}
										placeholder="hostname or IP address"
										class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									/>
								</div>
								<div class="w-32">
									<input
										type="number"
										bind:value={endpoint.port}
										min="1"
										max="65535"
										placeholder="Port"
										class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									/>
								</div>
								{#if formState.endpoints.length > 1}
									<button
										onclick={() => handleRemoveEndpoint(index)}
										class="p-2 text-red-600 hover:bg-red-50 rounded-md transition-colors"
										title="Remove endpoint"
									>
										<Trash2 class="h-4 w-4" />
									</button>
								{/if}
							</div>
						{/each}
					</div>
				</div>

				<!-- Load Balancing -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Load Balancing</h2>
					<div class="space-y-4">
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">
								Load Balancing Policy
							</label>
							<select
								bind:value={formState.lbPolicy}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							>
								<option value="ROUND_ROBIN">Round Robin</option>
								<option value="LEAST_REQUEST">Least Request</option>
								<option value="RANDOM">Random</option>
								<option value="RING_HASH">Ring Hash</option>
								<option value="MAGLEV">Maglev</option>
							</select>
							<p class="text-xs text-gray-500 mt-1">
								Algorithm for distributing requests across endpoints
							</p>
						</div>
					</div>
				</div>

				<!-- Resilience Configuration (Collapsible) -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 mb-6">
					<button
						onclick={() => (resilienceExpanded = !resilienceExpanded)}
						class="w-full px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
					>
						<div class="text-left">
							<h2 class="text-lg font-semibold text-gray-900">Resilience Configuration</h2>
							<p class="text-sm text-gray-600">Health checks, circuit breaker, and outlier detection</p>
						</div>
						{#if resilienceExpanded}
							<ChevronUp class="w-5 h-5 text-gray-500" />
						{:else}
							<ChevronDown class="w-5 h-5 text-gray-500" />
						{/if}
					</button>
					{#if resilienceExpanded}
						<div class="px-6 pb-6">
							<ClusterConfigEditor
								healthChecks={formState.healthChecks}
								circuitBreakers={formState.circuitBreakers}
								outlierDetection={formState.outlierDetection}
								onHealthChecksChange={handleHealthChecksChange}
								onCircuitBreakersChange={handleCircuitBreakersChange}
								onOutlierDetectionChange={handleOutlierDetectionChange}
							/>
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
						<div class="px-6 pb-6 space-y-4">
							<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
								<div>
									<label class="block text-sm font-medium text-gray-700 mb-1">
										Connect Timeout (seconds)
									</label>
									<input
										type="number"
										bind:value={formState.connectTimeout}
										min="1"
										placeholder="5"
										class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									/>
									<p class="text-xs text-gray-500 mt-1">
										Connection timeout in seconds
									</p>
								</div>

								<div>
									<label class="block text-sm font-medium text-gray-700 mb-1">
										DNS Lookup Family
									</label>
									<select
										bind:value={formState.dnsLookupFamily}
										class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									>
										<option value="">Auto (default)</option>
										<option value="V4_ONLY">IPv4 Only</option>
										<option value="V6_ONLY">IPv6 Only</option>
										<option value="V4_PREFERRED">IPv4 Preferred</option>
										<option value="ALL">All</option>
									</select>
									<p class="text-xs text-gray-500 mt-1">
										IP version preference for DNS resolution
									</p>
								</div>
							</div>
						</div>
					{/if}
				</div>

				<!-- Action Buttons -->
				<div class="sticky bottom-0 bg-white border-t border-gray-200 p-4 -mx-8 flex justify-end gap-3">
					<Button onclick={handleCancel} variant="secondary" disabled={isSubmitting}>
						Cancel
					</Button>
					<Button onclick={handleSubmit} variant="primary" disabled={isSubmitting}>
						{isSubmitting ? 'Updating...' : 'Update Cluster'}
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
{/if}
