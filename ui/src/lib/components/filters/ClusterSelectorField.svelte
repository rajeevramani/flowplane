<script lang="ts">
	import type { ClusterResponse } from '$lib/api/types';
	import ClusterSelector, { type ClusterConfig } from '../ClusterSelector.svelte';
	import { apiClient } from '$lib/api/client';
	import { onMount } from 'svelte';

	interface Props {
		/** The cluster name value (for existing cluster or new cluster name) */
		value: string;
		/** Callback when the cluster selection changes */
		onChange: (value: string) => void;
		/** Optional label for the field */
		label?: string;
		/** Optional description for the field */
		description?: string;
		/** Whether the field is required */
		required?: boolean;
		/** Optional error messages */
		errors?: string[];
	}

	let { value, onChange, label, description, required = false, errors = [] }: Props = $props();

	// State for available clusters
	let clusters = $state<ClusterResponse[]>([]);
	let loading = $state(true);
	let loadError = $state<string | null>(null);

	// Internal cluster config state
	let clusterConfig = $state<ClusterConfig>({
		mode: 'existing',
		existingClusterName: value || null
	});

	// Initialize clusterConfig based on value
	$effect(() => {
		if (value && clusters.length > 0) {
			// Check if value matches an existing cluster
			const existingCluster = clusters.find(c => c.name === value);
			if (existingCluster) {
				clusterConfig = {
					mode: 'existing',
					existingClusterName: value
				};
			} else {
				// Value doesn't match existing cluster, treat as new
				clusterConfig = {
					mode: 'new',
					existingClusterName: null,
					newClusterConfig: {
						name: value,
						endpoints: [{ host: '', port: 8080 }],
						lbPolicy: 'ROUND_ROBIN'
					}
				};
			}
		}
	});

	onMount(async () => {
		try {
			loading = true;
			clusters = await apiClient.listClusters();
		} catch (e) {
			loadError = e instanceof Error ? e.message : 'Failed to load clusters';
		} finally {
			loading = false;
		}
	});

	function handleConfigChange(newConfig: ClusterConfig) {
		clusterConfig = newConfig;

		// Extract the cluster name to pass back to the parent
		if (newConfig.mode === 'existing' && newConfig.existingClusterName) {
			onChange(newConfig.existingClusterName);
		} else if (newConfig.mode === 'new' && newConfig.newClusterConfig?.name) {
			onChange(newConfig.newClusterConfig.name);
		}
	}

	const hasError = $derived(errors.length > 0);
</script>

<div class="space-y-2">
	{#if label}
		<label class="flex items-center gap-1 text-sm font-medium text-gray-700">
			{label}
			{#if required}
				<span class="text-red-500">*</span>
			{/if}
		</label>
	{/if}

	{#if description}
		<p class="text-xs text-gray-500">{description}</p>
	{/if}

	{#if loading}
		<div class="flex items-center gap-2 text-sm text-gray-500">
			<svg class="animate-spin h-4 w-4" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
				<circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
				<path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
			</svg>
			Loading clusters...
		</div>
	{:else if loadError}
		<div class="text-sm text-red-600">
			{loadError}
		</div>
	{:else}
		<div class={hasError ? 'border border-red-300 rounded-md p-2' : ''}>
			<ClusterSelector
				{clusters}
				config={clusterConfig}
				onConfigChange={handleConfigChange}
			/>
		</div>
	{/if}

	{#if errors.length > 0}
		<div class="space-y-1">
			{#each errors as error}
				<p class="text-xs text-red-600">{error}</p>
			{/each}
		</div>
	{/if}
</div>
