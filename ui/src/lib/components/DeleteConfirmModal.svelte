<script lang="ts">
	import { AlertTriangle } from 'lucide-svelte';
	import Button from './Button.svelte';

	interface Props {
		show: boolean;
		resourceType: string;
		resourceName: string;
		onConfirm: () => void | Promise<void>;
		onCancel: () => void;
		loading?: boolean;
		warningMessage?: string;
	}

	let {
		show,
		resourceType,
		resourceName,
		onConfirm,
		onCancel,
		loading = false,
		warningMessage
	}: Props = $props();

	function handleBackdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget && !loading) {
			onCancel();
		}
	}

	function handleKeydown(event: KeyboardEvent) {
		if (event.key === 'Escape' && show && !loading) {
			onCancel();
		}
	}

	async function handleConfirm() {
		await onConfirm();
	}
</script>

<svelte:window onkeydown={handleKeydown} />

{#if show}
	<div
		class="fixed inset-0 bg-black/50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
		onclick={handleBackdropClick}
	>
		<div
			class="bg-white rounded-lg shadow-xl p-6 max-w-md w-full mx-4"
			onclick={(e) => e.stopPropagation()}
		>
			<!-- Warning Icon -->
			<div class="flex justify-center mb-4">
				<div class="h-12 w-12 rounded-full bg-red-100 flex items-center justify-center">
					<AlertTriangle class="h-6 w-6 text-red-600" />
				</div>
			</div>

			<!-- Title -->
			<h2 class="text-lg font-semibold text-gray-900 text-center mb-2">
				Delete {resourceType}?
			</h2>

			<!-- Warning Message -->
			<p class="text-sm text-gray-600 text-center mb-2">
				Are you sure you want to delete <span class="font-medium text-gray-900">"{resourceName}"</span>?
			</p>

			{#if warningMessage}
				<p class="text-sm text-red-600 text-center mb-4">
					{warningMessage}
				</p>
			{:else}
				<p class="text-sm text-gray-500 text-center mb-6">
					This action cannot be undone.
				</p>
			{/if}

			<!-- Actions -->
			<div class="flex justify-center gap-3">
				<Button variant="ghost" onclick={onCancel} disabled={loading}>
					Cancel
				</Button>
				<Button variant="danger" onclick={handleConfirm} disabled={loading}>
					{#if loading}
						<span class="flex items-center gap-2">
							<span class="animate-spin h-4 w-4 border-2 border-white border-t-transparent rounded-full"></span>
							Deleting...
						</span>
					{:else}
						Delete
					{/if}
				</Button>
			</div>
		</div>
	</div>
{/if}
