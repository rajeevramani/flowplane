<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { Plus, Trash2, ChevronDown, ChevronUp, ArrowLeft } from 'lucide-svelte';
	import { selectedTeam } from '$lib/stores/team';
	import type {
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
	let error = $state<string | null>(null);
	let isSubmitting = $state(false);
	let activeTab = $state<'configuration' | 'json'>('configuration');
	let resilienceExpanded = $state(false);
	let advancedExpanded = $state(false);

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Initialize form state
	let formState = $state<FormState>({
		name: '',
		team: currentTeam,
		serviceName: '',
		endpoints: [
			{
				id: `ep-${Date.now()}`,
				host: '',
				port: 8080
			}
		],
		lbPolicy: 'ROUND_ROBIN',
		healthChecks: [],
		circuitBreakers: null,
		outlierDetection: null
	});

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
		if (!/^[a-z0-9-]+$/.test(formState.name))
			return 'Name must be lowercase alphanumeric with dashes';
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
			console.log('Creating cluster:', payload);
			await apiClient.createCluster(payload);
			goto('/clusters');
		} catch (e) {
			console.error('Create failed:', e);
			error = e instanceof Error ? e.message : 'Failed to create cluster';
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
						<h1 class="text-3xl font-bold text-gray-900">Create Cluster</h1>
						<p class="text-sm text-gray-600 mt-1">
							Define a new backend service cluster
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
								placeholder="e.g., user-service-cluster"
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							<p class="text-xs text-gray-500 mt-1">
								Unique identifier (lowercase, alphanumeric, dashes only)
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

				{#if formState.endpoints.length === 0}
					<div class="border-2 border-dashed border-gray-300 rounded-lg p-8 text-center">
						<p class="text-sm text-gray-600 mb-3">No endpoints defined</p>
						<button
							onclick={handleAddEndpoint}
							class="px-4 py-2 text-sm text-blue-600 border border-blue-600 rounded-md hover:bg-blue-50 transition-colors"
						>
							<Plus class="h-4 w-4 inline mr-1" />
							Add Endpoint
						</button>
					</div>
				{/if}
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
					{isSubmitting ? 'Creating...' : 'Create Cluster'}
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
