<script lang="ts">
	/**
	 * Feature badges for displaying enabled features on resources.
	 * Badge colors per wireframe:
	 * - RT (Retry): Blue
	 * - CB (Circuit Breaker): Yellow
	 * - OD (Outlier Detection): Orange
	 * - HC (Health Check): Green
	 */

	interface Props {
		hasRetry?: boolean;
		hasCircuitBreaker?: boolean;
		hasOutlierDetection?: boolean;
		hasHealthCheck?: boolean;
	}

	let {
		hasRetry = false,
		hasCircuitBreaker = false,
		hasOutlierDetection = false,
		hasHealthCheck = false
	}: Props = $props();

	// Badge configuration
	const badges = [
		{
			id: 'retry',
			label: 'RT',
			title: 'Retry Policy',
			enabled: hasRetry,
			bgColor: 'bg-blue-500',
			textColor: 'text-white'
		},
		{
			id: 'circuitBreaker',
			label: 'CB',
			title: 'Circuit Breaker',
			enabled: hasCircuitBreaker,
			bgColor: 'bg-yellow-500',
			textColor: 'text-white'
		},
		{
			id: 'outlierDetection',
			label: 'OD',
			title: 'Outlier Detection',
			enabled: hasOutlierDetection,
			bgColor: 'bg-orange-500',
			textColor: 'text-white'
		},
		{
			id: 'healthCheck',
			label: 'HC',
			title: 'Health Check',
			enabled: hasHealthCheck,
			bgColor: 'bg-green-500',
			textColor: 'text-white'
		}
	];

	// Filter to only show enabled badges
	let enabledBadges = $derived(badges.filter((b) => b.enabled));
</script>

{#if enabledBadges.length > 0}
	<div class="flex items-center gap-1">
		{#each enabledBadges as badge}
			<span
				title={badge.title}
				class="inline-flex items-center justify-center px-1.5 py-0.5 text-xs font-semibold rounded {badge.bgColor} {badge.textColor}"
			>
				{badge.label}
			</span>
		{/each}
	</div>
{:else}
	<span class="text-gray-400 text-sm">-</span>
{/if}
