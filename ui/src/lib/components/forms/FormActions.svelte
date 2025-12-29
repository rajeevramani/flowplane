<script lang="ts">
	import { Loader2 } from 'lucide-svelte';
	import Button from '$lib/components/Button.svelte';

	interface Props {
		isSubmitting: boolean;
		submitLabel: string;
		submittingLabel?: string;
		cancelLabel?: string;
		onSubmit: () => void;
		onCancel: () => void;
		variant?: 'default' | 'sticky';
		class?: string;
	}

	let {
		isSubmitting,
		submitLabel,
		submittingLabel,
		cancelLabel = 'Cancel',
		onSubmit,
		onCancel,
		variant = 'default',
		class: className = ''
	}: Props = $props();

	let submittingText = $derived(submittingLabel ?? 'Saving...');
</script>

{#if variant === 'sticky'}
	<div class="sticky bottom-0 bg-white border-t border-gray-200 p-4 -mx-8 flex justify-end gap-3 {className}">
		<Button onclick={onCancel} variant="secondary" disabled={isSubmitting}>
			{cancelLabel}
		</Button>
		<Button onclick={onSubmit} variant="primary" disabled={isSubmitting}>
			{#if isSubmitting}
				<Loader2 class="h-4 w-4 mr-2 animate-spin" />
				{submittingText}
			{:else}
				{submitLabel}
			{/if}
		</Button>
	</div>
{:else}
	<div class="flex justify-end gap-3 {className}">
		<Button onclick={onCancel} variant="secondary" disabled={isSubmitting}>
			{cancelLabel}
		</Button>
		<Button onclick={onSubmit} variant="primary" disabled={isSubmitting}>
			{#if isSubmitting}
				<Loader2 class="h-4 w-4 mr-2 animate-spin" />
				{submittingText}
			{:else}
				{submitLabel}
			{/if}
		</Button>
	</div>
{/if}
