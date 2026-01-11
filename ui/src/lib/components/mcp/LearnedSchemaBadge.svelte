<script lang="ts">
	import Badge from '../Badge.svelte';
	import type { LearnedSchemaInfo } from '$lib/api/types';

	interface Props {
		schema: LearnedSchemaInfo;
	}

	let { schema }: Props = $props();

	let confidencePercent = $derived(Math.round(schema.confidenceScore * 100));
	let confidenceColor = $derived(
		confidencePercent >= 80
			? 'text-emerald-600'
			: confidencePercent >= 50
				? 'text-yellow-600'
				: 'text-red-600'
	);
</script>

<div class="flex items-center gap-2 text-sm">
	<Badge variant="purple">Learned Schema</Badge>
	<span class={confidenceColor}>{confidencePercent}%</span>
	<span class="text-gray-500">{schema.sampleCount} samples</span>
</div>
