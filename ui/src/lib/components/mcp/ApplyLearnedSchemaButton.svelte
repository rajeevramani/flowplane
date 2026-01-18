<script lang="ts">
	import { CheckCircle2, Sparkles } from 'lucide-svelte';
	import Badge from '../Badge.svelte';
	import Button from '../Button.svelte';
	import type { McpTool } from '$lib/api/types';

	interface Props {
		tool: McpTool;
		onApply: () => void;
		disabled?: boolean;
	}

	let { tool, onApply, disabled = false }: Props = $props();

	// Determine if we should show the button based on schema source and confidence
	let shouldShow = $derived(
		tool.schemaSource === 'learned' &&
		tool.confidence !== null &&
		tool.confidence >= 0.8
	);

	let confidencePercent = $derived(
		tool.confidence !== null ? Math.round(tool.confidence * 100) : 0
	);

	// Badge variant based on confidence level
	type BadgeVariant = 'blue' | 'purple' | 'green' | 'red' | 'yellow' | 'orange' | 'indigo' | 'gray';
	let confidenceBadgeVariant = $derived<BadgeVariant>(
		confidencePercent >= 95 ? 'green' : confidencePercent >= 85 ? 'blue' : 'yellow'
	);
</script>

{#if shouldShow}
	<div class="flex items-center gap-2">
		<Button variant="secondary" size="sm" onclick={onApply} {disabled}>
			<Sparkles class="w-4 h-4 mr-2" />
			Apply Learned Schema
		</Button>
		<div class="flex items-center gap-2">
			<Badge variant={confidenceBadgeVariant} size="sm">
				<CheckCircle2 class="w-3 h-3 mr-1" />
				{confidencePercent}% confidence
			</Badge>
		</div>
	</div>
{/if}
