<script lang="ts">
	import type { DomainGroupData } from './DomainGroup.svelte';

	interface Props {
		show: boolean;
		group: DomainGroupData | null;
		onSave: (domains: string[]) => void;
		onCancel: () => void;
	}

	let { show, group, onSave, onCancel }: Props = $props();

	let domainsInput = $state('');

	// Reset form when group changes
	$effect(() => {
		if (show) {
			if (group) {
				// Editing existing domain group
				domainsInput = group.domains.join(', ');
			} else {
				// Creating new domain group
				domainsInput = '';
			}
		}
	});

	function handleSave() {
		// Parse comma-separated domains, trim whitespace, filter empty
		const domains = domainsInput
			.split(',')
			.map((d) => d.trim())
			.filter((d) => d.length > 0);

		if (domains.length === 0) {
			return; // Don't save empty domains
		}

		onSave(domains);
	}

	function handleBackdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget) {
			onCancel();
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
			class="bg-white rounded-lg shadow-xl w-full max-w-md mx-4"
			onclick={(e) => e.stopPropagation()}
		>
			<!-- Header -->
			<div class="px-6 py-4 border-b border-gray-200 flex items-center justify-between">
				<h2 class="text-lg font-semibold text-gray-900">
					{group ? 'Edit Domain' : 'Add Domain'}
				</h2>
				<button
					type="button"
					onclick={onCancel}
					class="p-1 text-gray-400 hover:text-gray-600 rounded"
				>
					<svg class="h-6 w-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							stroke-width="2"
							d="M6 18L18 6M6 6l12 12"
						/>
					</svg>
				</button>
			</div>

			<!-- Body -->
			<div class="px-6 py-4">
				<div>
					<label for="domain-input" class="block text-sm font-medium text-gray-700 mb-1"
						>Domain(s)</label
					>
					<input
						id="domain-input"
						type="text"
						bind:value={domainsInput}
						placeholder="api.example.com, *.example.com"
						class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
					/>
					<p class="mt-1 text-xs text-gray-500">
						Comma-separated for multiple domains with same routes. Use * as wildcard.
					</p>
				</div>
			</div>

			<!-- Footer -->
			<div class="px-6 py-4 border-t border-gray-200 flex justify-end gap-3">
				<button
					type="button"
					onclick={onCancel}
					class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
				>
					Cancel
				</button>
				<button
					type="button"
					onclick={handleSave}
					class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
				>
					{group ? 'Save Changes' : 'Create Domain'}
				</button>
			</div>
		</div>
	</div>
{/if}
