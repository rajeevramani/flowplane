<script lang="ts">
	import { X } from 'lucide-svelte';
	import type { Snippet } from 'svelte';

	interface Props {
		open: boolean;
		title: string;
		subtitle?: string;
		onClose: () => void;
		children: Snippet;
		footer?: Snippet;
	}

	let { open, title, subtitle, onClose, children, footer }: Props = $props();

	// Handle escape key
	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && open) {
			onClose();
		}
	}

	// Handle backdrop click
	function handleBackdropClick(e: MouseEvent) {
		if (e.target === e.currentTarget) {
			onClose();
		}
	}
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
	<!-- Backdrop -->
	<div
		class="fixed inset-0 bg-black/50 z-40 transition-opacity"
		onclick={handleBackdropClick}
		role="button"
		tabindex="-1"
		aria-label="Close drawer"
	></div>

	<!-- Drawer -->
	<div
		class="fixed inset-y-0 right-0 w-full max-w-lg bg-white shadow-xl z-50 flex flex-col transform transition-transform duration-300 ease-out"
		class:translate-x-0={open}
		class:translate-x-full={!open}
		role="dialog"
		aria-modal="true"
		aria-labelledby="drawer-title"
	>
		<!-- Header -->
		<div class="flex items-start justify-between px-6 py-4 border-b border-gray-200">
			<div>
				<h2 id="drawer-title" class="text-lg font-semibold text-gray-900">{title}</h2>
				{#if subtitle}
					<p class="mt-1 text-sm text-gray-500">{subtitle}</p>
				{/if}
			</div>
			<button
				onclick={onClose}
				class="p-1 rounded-md text-gray-400 hover:text-gray-600 hover:bg-gray-100 transition-colors"
				aria-label="Close"
			>
				<X class="h-5 w-5" />
			</button>
		</div>

		<!-- Content -->
		<div class="flex-1 overflow-y-auto px-6 py-4">
			{@render children()}
		</div>

		<!-- Footer (optional) -->
		{#if footer}
			<div class="px-6 py-4 border-t border-gray-200 bg-gray-50">
				{@render footer()}
			</div>
		{/if}
	</div>
{/if}
