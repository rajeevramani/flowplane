<script lang="ts">
	import Button from './Button.svelte';

	interface Props {
		show: boolean;
		title: string;
		onClose: () => void;
		onConfirm?: () => void;
		confirmText?: string;
		cancelText?: string;
		confirmVariant?: 'primary' | 'danger';
		children: any;
	}

	let {
		show,
		title,
		onClose,
		onConfirm,
		confirmText = 'Confirm',
		cancelText = 'Cancel',
		confirmVariant = 'primary',
		children
	}: Props = $props();

	function handleBackdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget) {
			onClose();
		}
	}
</script>

{#if show}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
		onclick={handleBackdropClick}
	>
		<div
			class="bg-white rounded-lg shadow-xl p-6 max-w-md w-full mx-4"
			onclick={(e) => e.stopPropagation()}
		>
			<h2 class="text-lg font-semibold text-gray-900 mb-4">{title}</h2>
			<div class="mb-6">
				{@render children()}
			</div>
			<div class="flex justify-end gap-3">
				<Button variant="ghost" onclick={onClose}>
					{cancelText}
				</Button>
				{#if onConfirm}
					<Button variant={confirmVariant} onclick={onConfirm}>
						{confirmText}
					</Button>
				{/if}
			</div>
		</div>
	</div>
{/if}
