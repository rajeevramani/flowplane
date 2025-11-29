<script lang="ts">
	interface Props {
		position?: 'top' | 'bottom' | 'left' | 'right';
		children: any;
		tooltip?: any;
	}

	let { position = 'bottom', children, tooltip }: Props = $props();

	let showTooltip = $state(false);

	const positionClasses = {
		top: 'bottom-full left-1/2 -translate-x-1/2 mb-2',
		bottom: 'top-full left-1/2 -translate-x-1/2 mt-2',
		left: 'right-full top-1/2 -translate-y-1/2 mr-2',
		right: 'left-full top-1/2 -translate-y-1/2 ml-2'
	};
</script>

<div
	class="relative inline-block"
	onmouseenter={() => (showTooltip = true)}
	onmouseleave={() => (showTooltip = false)}
>
	{@render children()}

	{#if showTooltip && tooltip}
		<div
			class="absolute z-50 {positionClasses[position]} pointer-events-none"
		>
			<div class="bg-white border border-gray-200 rounded-lg shadow-lg p-3 text-sm whitespace-nowrap">
				{@render tooltip()}
			</div>
		</div>
	{/if}
</div>
