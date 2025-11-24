<script lang="ts">
	import type { ClusterResponse, EndpointRequest } from '$lib/api/types';
	import EndpointList from './EndpointList.svelte';

	export interface NewClusterConfig {
		name: string;
		endpoints: EndpointRequest[];
		lbPolicy: string;
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
						<option value={cluster.name}>{cluster.name}</option>
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
		</div>
	{/if}
</div>
