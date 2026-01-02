<script lang="ts">
	interface Props {
		current: number;
		target: number;
		showLabel?: boolean;
		size?: 'sm' | 'md' | 'lg';
		animated?: boolean;
	}

	let { current, target, showLabel = true, size = 'md', animated = false }: Props = $props();

	let percentage = $derived(target > 0 ? Math.min((current / target) * 100, 100) : 0);

	let barColor = $derived.by(() => {
		if (percentage >= 100) return 'bg-green-500';
		if (percentage >= 75) return 'bg-blue-500';
		if (percentage >= 50) return 'bg-blue-400';
		return 'bg-blue-300';
	});

	const sizeClasses = {
		sm: 'h-1.5',
		md: 'h-2',
		lg: 'h-3'
	};
</script>

<div class="w-full">
	{#if showLabel}
		<div class="flex justify-between mb-1">
			<span class="text-sm text-gray-600">
				{current.toLocaleString()} / {target.toLocaleString()} samples
			</span>
			<span class="text-sm font-medium text-gray-700">
				{percentage.toFixed(1)}%
			</span>
		</div>
	{/if}
	<div class="w-full bg-gray-200 rounded-full overflow-hidden {sizeClasses[size]}">
		<div
			class="h-full rounded-full transition-all duration-500 ease-out {barColor} {animated
				? 'animate-pulse'
				: ''}"
			style="width: {percentage}%"
		></div>
	</div>
</div>
