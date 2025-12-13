<script lang="ts">
	import type {
		ClusterResponse,
		EndpointRequest,
		HealthCheckRequest,
		CircuitBreakersRequest,
		OutlierDetectionRequest
	} from '$lib/api/types';
	import EndpointList from './EndpointList.svelte';
	import ClusterConfigEditor from './ClusterConfigEditor.svelte';

	export interface NewClusterConfig {
		name: string;
		endpoints: EndpointRequest[];
		lbPolicy: string;
		healthChecks?: HealthCheckRequest[];
		circuitBreakers?: CircuitBreakersRequest;
		outlierDetection?: OutlierDetectionRequest;
	}

	export interface ClusterConfig {
		mode: 'existing' | 'new';
		existingClusterName: string | null;
		newClusterConfig?: NewClusterConfig;
	}

	interface Props {
		clusters: ClusterResponse[];
		config: ClusterConfig;
		onConfigChange: (config: ClusterConfig) => void;
	}

	let { clusters, config, onConfigChange }: Props = $props();

	// State for advanced configuration section
	let showAdvancedConfig = $state(false);

	// Derived: check if any advanced config is set
	let hasAdvancedConfig = $derived(
		config.mode === 'new' &&
		config.newClusterConfig &&
		(
			(config.newClusterConfig.healthChecks?.length ?? 0) > 0 ||
			config.newClusterConfig.circuitBreakers !== undefined ||
			config.newClusterConfig.outlierDetection !== undefined
		)
	);

	// Initialize new cluster config if not present
	$effect(() => {
		if (config.mode === 'new' && !config.newClusterConfig) {
			onConfigChange({
				...config,
				newClusterConfig: {
					name: '',
					endpoints: [{ host: '', port: 8080 }],
					lbPolicy: 'ROUND_ROBIN'
				}
			});
		}
	});

	function handleModeChange(mode: 'existing' | 'new') {
		if (mode === 'new') {
			onConfigChange({
				mode: 'new',
				existingClusterName: null,
				newClusterConfig: config.newClusterConfig || {
					name: '',
					endpoints: [{ host: '', port: 8080 }],
					lbPolicy: 'ROUND_ROBIN'
				}
			});
		} else {
			onConfigChange({
				mode: 'existing',
				existingClusterName: config.existingClusterName
			});
		}
	}

	function handleExistingClusterChange(e: Event) {
		const target = e.target as HTMLSelectElement;
		onConfigChange({
			...config,
			existingClusterName: target.value || null
		});
	}

	function handleNewClusterNameChange(e: Event) {
		const target = e.target as HTMLInputElement;
		onConfigChange({
			...config,
			newClusterConfig: {
				...config.newClusterConfig!,
				name: target.value
			}
		});
	}

	function handleEndpointsChange(endpoints: EndpointRequest[]) {
		onConfigChange({
			...config,
			newClusterConfig: {
				...config.newClusterConfig!,
				endpoints
			}
		});
	}

	function handleLbPolicyChange(lbPolicy: string) {
		onConfigChange({
			...config,
			newClusterConfig: {
				...config.newClusterConfig!,
				lbPolicy
			}
		});
	}

	function handleHealthChecksChange(healthChecks: HealthCheckRequest[]) {
		onConfigChange({
			...config,
			newClusterConfig: {
				...config.newClusterConfig!,
				healthChecks: healthChecks.length > 0 ? healthChecks : undefined
			}
		});
	}

	function handleCircuitBreakersChange(circuitBreakers: CircuitBreakersRequest | null) {
		onConfigChange({
			...config,
			newClusterConfig: {
				...config.newClusterConfig!,
				circuitBreakers: circuitBreakers ?? undefined
			}
		});
	}

	function handleOutlierDetectionChange(outlierDetection: OutlierDetectionRequest | null) {
		onConfigChange({
			...config,
			newClusterConfig: {
				...config.newClusterConfig!,
				outlierDetection: outlierDetection ?? undefined
			}
		});
	}

	// Get primary host from cluster config for display
	function getClusterDisplayInfo(cluster: ClusterResponse): string {
		const endpoints = cluster.config?.endpoints;
		if (!endpoints || !Array.isArray(endpoints) || endpoints.length === 0) {
			return cluster.name;
		}
		const firstEndpoint = endpoints[0];
		const host = firstEndpoint.host || firstEndpoint.address;
		const port = firstEndpoint.port;
		if (host) {
			const hostDisplay = port ? `${host}:${port}` : host;
			const moreCount = endpoints.length > 1 ? ` (+${endpoints.length - 1} more)` : '';
			return `${cluster.name} → ${hostDisplay}${moreCount}`;
		}
		return cluster.name;
	}
</script>

<div class="space-y-3">
	<!-- Existing Cluster Option -->
	<label class="flex items-center gap-3 cursor-pointer">
		<input
			type="radio"
			name="cluster-mode"
			checked={config.mode === 'existing'}
			onchange={() => handleModeChange('existing')}
			class="h-4 w-4 text-blue-600 focus:ring-blue-500"
		/>
		<span class="text-sm text-gray-700">Use existing cluster</span>
	</label>

	{#if config.mode === 'existing'}
		<div class="ml-7">
			{#if clusters.length === 0}
				<p class="text-sm text-gray-500 italic">No existing clusters available</p>
			{:else}
				<select
					value={config.existingClusterName ?? ''}
					onchange={handleExistingClusterChange}
					class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
				>
					<option value="">Select a cluster...</option>
					{#each clusters as cluster}
						<option value={cluster.name}>{getClusterDisplayInfo(cluster)}</option>
					{/each}
				</select>
			{/if}
		</div>
	{/if}

	<!-- New Cluster Option -->
	<label class="flex items-center gap-3 cursor-pointer">
		<input
			type="radio"
			name="cluster-mode"
			checked={config.mode === 'new'}
			onchange={() => handleModeChange('new')}
			class="h-4 w-4 text-blue-600 focus:ring-blue-500"
		/>
		<span class="text-sm text-gray-700">Create new cluster</span>
	</label>

	{#if config.mode === 'new' && config.newClusterConfig}
		<div class="ml-7 space-y-4 p-4 bg-gray-50 rounded-lg border border-gray-200">
			<!-- Cluster Name -->
			<div>
				<label for="new-cluster-name" class="block text-sm font-medium text-gray-700 mb-1"
					>Cluster Name</label
				>
				<input
					id="new-cluster-name"
					type="text"
					value={config.newClusterConfig.name}
					oninput={handleNewClusterNameChange}
					placeholder="my-backend-cluster"
					class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
				/>
			</div>

			<!-- Endpoints -->
			<EndpointList
				endpoints={config.newClusterConfig.endpoints}
				lbPolicy={config.newClusterConfig.lbPolicy}
				onEndpointsChange={handleEndpointsChange}
				onLbPolicyChange={handleLbPolicyChange}
			/>

			<!-- Advanced Configuration (collapsible) -->
			<div class="border-t border-gray-200 pt-3 mt-3">
				<button
					type="button"
					onclick={() => showAdvancedConfig = !showAdvancedConfig}
					class="flex items-center gap-2 text-sm font-medium text-gray-700 hover:text-gray-900 w-full"
				>
					<svg
						class="h-4 w-4 transition-transform {showAdvancedConfig ? 'rotate-90' : ''}"
						fill="none"
						stroke="currentColor"
						viewBox="0 0 24 24"
					>
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							stroke-width="2"
							d="M9 5l7 7-7 7"
						/>
					</svg>
					Advanced Configuration
					{#if hasAdvancedConfig}
						<span class="ml-1 inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium bg-blue-100 text-blue-700">
							Configured
						</span>
					{/if}
				</button>

				{#if showAdvancedConfig}
					<div class="mt-3 p-3 bg-white rounded-lg border border-gray-200">
						<ClusterConfigEditor
							healthChecks={config.newClusterConfig.healthChecks || []}
							circuitBreakers={config.newClusterConfig.circuitBreakers || null}
							outlierDetection={config.newClusterConfig.outlierDetection || null}
							onHealthChecksChange={handleHealthChecksChange}
							onCircuitBreakersChange={handleCircuitBreakersChange}
							onOutlierDetectionChange={handleOutlierDetectionChange}
							compact={true}
						/>
					</div>
				{/if}
			</div>
		</div>
	{/if}
</div>
