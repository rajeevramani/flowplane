<script lang="ts">
	/**
	 * Status indicator with colored dot and label.
	 * - green: Active/Healthy
	 * - red: Error/Unhealthy
	 * - yellow: Warning/Degraded
	 * - gray: Unknown/Pending
	 */

	type StatusType = 'active' | 'error' | 'warning' | 'unknown';

	interface Props {
		status: StatusType;
		label?: string;
		showLabel?: boolean;
	}

	let { status = 'unknown', label, showLabel = true }: Props = $props();

	const statusConfig: Record<StatusType, { color: string; defaultLabel: string }> = {
		active: { color: 'bg-green-500', defaultLabel: 'Active' },
		error: { color: 'bg-red-500', defaultLabel: 'Error' },
		warning: { color: 'bg-yellow-500', defaultLabel: 'Warning' },
		unknown: { color: 'bg-gray-400', defaultLabel: 'Unknown' }
	};

	let config = $derived(statusConfig[status] || statusConfig.unknown);
	let displayLabel = $derived(label || config.defaultLabel);
</script>

<div class="flex items-center gap-2">
	<span class="relative flex h-2.5 w-2.5">
		<span
			class="animate-ping absolute inline-flex h-full w-full rounded-full opacity-75 {status ===
			'active'
				? 'bg-green-400'
				: ''}"
		></span>
		<span class="relative inline-flex rounded-full h-2.5 w-2.5 {config.color}"></span>
	</span>
	{#if showLabel}
		<span class="text-sm text-gray-700">{displayLabel}</span>
	{/if}
</div>
