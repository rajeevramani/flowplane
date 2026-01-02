<script lang="ts">
	import { AlertCircle } from 'lucide-svelte';

	interface Props {
		message: string | null;
		variant?: 'default' | 'with-icon';
		class?: string;
	}

	let { message, variant = 'default', class: className = '' }: Props = $props();
</script>

{#if message}
	<div
		class="mb-6 bg-red-50 border border-red-200 rounded-md p-4 {className}"
		role="alert"
	>
		{#if variant === 'with-icon'}
			<div class="flex items-start gap-3">
				<AlertCircle class="h-5 w-5 text-red-600 flex-shrink-0 mt-0.5" />
				<p class="text-sm text-red-800">{message}</p>
			</div>
		{:else}
			<p class="text-sm text-red-800">{message}</p>
		{/if}
	</div>
{/if}
