<script lang="ts">
	import { ArrowLeft } from 'lucide-svelte';

	interface Props {
		title: string;
		subtitle?: string;
		onBack: () => void;
		variant?: 'compact' | 'large';
		class?: string;
	}

	let {
		title,
		subtitle,
		onBack,
		variant = 'compact',
		class: className = ''
	}: Props = $props();
</script>

<div class="mb-6 {className}">
	<div class="flex items-center gap-4 mb-2">
		<button
			onclick={onBack}
			class="p-2 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded-md transition-colors"
			title="Go back"
		>
			{#if variant === 'large'}
				<ArrowLeft class="w-6 h-6" />
			{:else}
				<ArrowLeft class="w-5 h-5" />
			{/if}
		</button>
		<div>
			{#if variant === 'large'}
				<h1 class="text-3xl font-bold text-gray-900">{title}</h1>
			{:else}
				<h1 class="text-2xl font-bold text-gray-900">{title}</h1>
			{/if}
			{#if subtitle}
				<p class="mt-1 text-sm text-gray-600">{subtitle}</p>
			{/if}
		</div>
	</div>
</div>
