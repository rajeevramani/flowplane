<script lang="ts">
	import type {
		HealthCheckRequest,
		CircuitBreakersRequest,
		OutlierDetectionRequest
	} from '$lib/api/types';
	import HealthCheckList from './cluster/HealthCheckList.svelte';
	import CircuitBreakerForm from './cluster/CircuitBreakerForm.svelte';
	import OutlierDetectionForm from './cluster/OutlierDetectionForm.svelte';

	interface Props {
		// Data
		healthChecks: HealthCheckRequest[];
		circuitBreakers: CircuitBreakersRequest | null;
		outlierDetection: OutlierDetectionRequest | null;

		// Callbacks
		onHealthChecksChange: (checks: HealthCheckRequest[]) => void;
		onCircuitBreakersChange: (cb: CircuitBreakersRequest | null) => void;
		onOutlierDetectionChange: (od: OutlierDetectionRequest | null) => void;

		// Options
		compact?: boolean; // For embedded use in ClusterSelector
	}

	let {
		healthChecks,
		circuitBreakers,
		outlierDetection,
		onHealthChecksChange,
		onCircuitBreakersChange,
		onOutlierDetectionChange,
		compact = false
	}: Props = $props();

	type ConfigTab = 'healthCheck' | 'circuitBreaker' | 'outlierDetection';
	let activeTab = $state<ConfigTab>('healthCheck');

	// Derived: check if each config is configured
	let hasHealthCheck = $derived(healthChecks.length > 0);
	let hasCircuitBreaker = $derived(circuitBreakers !== null && circuitBreakers.default !== undefined);
	let hasOutlierDetection = $derived(outlierDetection !== null);

	// Tab definitions
	const tabs: { id: ConfigTab; label: string; badge: string; color: string }[] = [
		{ id: 'healthCheck', label: 'Health Check', badge: 'HC', color: 'green' },
		{ id: 'circuitBreaker', label: 'Circuit Breaker', badge: 'CB', color: 'yellow' },
		{ id: 'outlierDetection', label: 'Outlier Detection', badge: 'OD', color: 'orange' }
	];

	function isConfigured(tabId: ConfigTab): boolean {
		switch (tabId) {
			case 'healthCheck':
				return hasHealthCheck;
			case 'circuitBreaker':
				return hasCircuitBreaker;
			case 'outlierDetection':
				return hasOutlierDetection;
		}
	}

	function getBadgeClasses(color: string): string {
		switch (color) {
			case 'green':
				return 'bg-green-500 text-white';
			case 'yellow':
				return 'bg-yellow-500 text-white';
			case 'orange':
				return 'bg-orange-500 text-white';
			default:
				return 'bg-gray-500 text-white';
		}
	}
</script>

<div class="{compact ? 'space-y-3' : 'space-y-4'}">
	<!-- Tab Navigation -->
	<div class="flex border-b border-gray-200 {compact ? 'gap-1' : ''}">
		{#each tabs as tab}
			<button
				type="button"
				onclick={() => activeTab = tab.id}
				class="px-3 py-2 text-sm font-medium border-b-2 -mb-px transition-colors flex items-center gap-1.5
					{activeTab === tab.id
						? 'text-blue-600 border-blue-600'
						: 'text-gray-500 border-transparent hover:text-gray-700 hover:border-gray-300'}
					{compact ? 'px-2 py-1.5 text-xs' : ''}"
			>
				{#if compact}
					{tab.badge}
				{:else}
					{tab.label}
				{/if}
				{#if isConfigured(tab.id)}
					<span class="inline-flex items-center justify-center {compact ? 'w-4 h-4 text-[10px]' : 'px-1.5 py-0.5 text-xs'} font-semibold rounded {getBadgeClasses(tab.color)}">
						{#if compact}
							&#10003;
						{:else}
							{tab.badge}
						{/if}
					</span>
				{/if}
			</button>
		{/each}
	</div>

	<!-- Tab Content -->
	<div class="{compact ? 'min-h-[200px]' : 'min-h-[300px]'}">
		{#if activeTab === 'healthCheck'}
			<HealthCheckList
				checks={healthChecks}
				onChecksChange={onHealthChecksChange}
			/>
		{:else if activeTab === 'circuitBreaker'}
			<CircuitBreakerForm
				config={circuitBreakers}
				onConfigChange={onCircuitBreakersChange}
			/>
		{:else if activeTab === 'outlierDetection'}
			<OutlierDetectionForm
				config={outlierDetection}
				onConfigChange={onOutlierDetectionChange}
			/>
		{/if}
	</div>
</div>
