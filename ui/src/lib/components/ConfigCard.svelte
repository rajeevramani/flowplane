<script lang="ts">
	import { ChevronDown, ChevronRight } from 'lucide-svelte';
	import type { Snippet } from 'svelte';

	type VariantType = 'blue' | 'yellow' | 'orange' | 'green' | 'gray';

	interface Props {
		title: string;
		variant?: VariantType;
		collapsible?: boolean;
		defaultCollapsed?: boolean;
		children: Snippet;
	}

	let {
		title,
		variant = 'gray',
		collapsible = false,
		defaultCollapsed = false,
		children
	}: Props = $props();

	let collapsed = $state(defaultCollapsed);

	const variantStyles: Record<VariantType, { border: string; bg: string; header: string }> = {
		blue: {
			border: 'border-blue-200',
			bg: 'bg-blue-50',
			header: 'bg-blue-100 text-blue-800'
		},
		yellow: {
			border: 'border-yellow-200',
			bg: 'bg-yellow-50',
			header: 'bg-yellow-100 text-yellow-800'
		},
		orange: {
			border: 'border-orange-200',
			bg: 'bg-orange-50',
			header: 'bg-orange-100 text-orange-800'
		},
		green: {
			border: 'border-green-200',
			bg: 'bg-green-50',
			header: 'bg-green-100 text-green-800'
		},
		gray: {
			border: 'border-gray-200',
			bg: 'bg-gray-50',
			header: 'bg-gray-100 text-gray-800'
		}
	};

	let styles = $derived(variantStyles[variant]);

	function toggleCollapsed() {
		if (collapsible) {
			collapsed = !collapsed;
		}
	}
</script>

<div class="rounded-lg border {styles.border} overflow-hidden">
	<!-- Header -->
	{#if collapsible}
		<button
			onclick={toggleCollapsed}
			class="w-full flex items-center justify-between px-4 py-2.5 {styles.header} text-sm font-medium transition-colors hover:opacity-90"
		>
			<span>{title}</span>
			{#if collapsed}
				<ChevronRight class="h-4 w-4" />
			{:else}
				<ChevronDown class="h-4 w-4" />
			{/if}
		</button>
	{:else}
		<div class="px-4 py-2.5 {styles.header} text-sm font-medium">
			{title}
		</div>
	{/if}

	<!-- Content -->
	{#if !collapsed}
		<div class="p-4 {styles.bg}">
			{@render children()}
		</div>
	{/if}
</div>
