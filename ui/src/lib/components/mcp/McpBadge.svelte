<script lang="ts">
	import Badge from '../Badge.svelte';

	interface Props {
		status: 'enabled' | 'ready' | 'not_ready';
	}

	let { status }: Props = $props();

	let config = $derived(
		status === 'enabled'
			? { label: 'MCP Enabled', variant: 'green' as const }
			: status === 'ready'
				? { label: 'Ready', variant: 'blue' as const }
				: { label: 'Not Ready', variant: 'gray' as const }
	);
</script>

<Badge variant={config.variant}>
	{#if status === 'enabled'}
		<svg class="w-3 h-3 mr-1" fill="none" stroke="currentColor" viewBox="0 0 24 24">
			<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
		</svg>
	{/if}
	{config.label}
</Badge>
