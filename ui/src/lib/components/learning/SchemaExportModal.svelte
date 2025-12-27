<script lang="ts">
	import { X, Download, FileJson, Square, CheckSquare } from 'lucide-svelte';
	import type { AggregatedSchemaResponse } from '$lib/api/types';
	import { apiClient } from '$lib/api/client';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	interface Props {
		isOpen: boolean;
		schemas: AggregatedSchemaResponse[];
		onClose: () => void;
	}

	let { isOpen, schemas, onClose }: Props = $props();

	// Form state
	let selectedIds = $state<Set<number>>(new Set());
	let title = $state('Learned API');
	let version = $state('1.0.0');
	let description = $state('');
	let includeMetadata = $state(true);
	let isExporting = $state(false);
	let error = $state<string | null>(null);

	// Reset state when modal opens
	$effect(() => {
		if (isOpen) {
			selectedIds = new Set();
			title = 'Learned API';
			version = '1.0.0';
			description = '';
			includeMetadata = true;
			error = null;
		}
	});

	// Computed
	let allSelected = $derived(selectedIds.size === schemas.length && schemas.length > 0);
	let someSelected = $derived(selectedIds.size > 0 && selectedIds.size < schemas.length);
	let canExport = $derived(selectedIds.size > 0 && title.trim() !== '' && version.trim() !== '');

	function toggleSchema(id: number) {
		const newSet = new Set(selectedIds);
		if (newSet.has(id)) {
			newSet.delete(id);
		} else {
			newSet.add(id);
		}
		selectedIds = newSet;
	}

	function toggleAll() {
		if (allSelected) {
			selectedIds = new Set();
		} else {
			selectedIds = new Set(schemas.map((s) => s.id));
		}
	}

	async function handleExport() {
		if (!canExport) return;

		isExporting = true;
		error = null;

		try {
			const openapi = await apiClient.exportMultipleSchemasAsOpenApi({
				schemaIds: Array.from(selectedIds),
				title: title.trim(),
				version: version.trim(),
				description: description.trim() || undefined,
				includeMetadata
			});

			// Download as file
			const blob = new Blob([JSON.stringify(openapi, null, 2)], { type: 'application/json' });
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `${title.toLowerCase().replace(/\s+/g, '-')}-${version}.openapi.json`;
			a.click();
			URL.revokeObjectURL(url);

			onClose();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to export schemas';
		} finally {
			isExporting = false;
		}
	}

	function handleBackdropClick(e: MouseEvent) {
		if (e.target === e.currentTarget && !isExporting) {
			onClose();
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && !isExporting) {
			onClose();
		}
	}

	// Method color helper
	const methodColors: Record<string, 'blue' | 'green' | 'yellow' | 'red' | 'purple' | 'gray'> = {
		GET: 'blue',
		POST: 'green',
		PUT: 'yellow',
		DELETE: 'red',
		PATCH: 'purple'
	};
</script>

<svelte:window onkeydown={handleKeydown} />

{#if isOpen}
	<div
		class="fixed inset-0 bg-black/50 z-50 flex items-center justify-center"
		onclick={handleBackdropClick}
		role="dialog"
		aria-modal="true"
	>
		<div
			class="bg-white rounded-lg shadow-xl w-full max-w-2xl mx-4 max-h-[85vh] flex flex-col"
			onclick={(e) => e.stopPropagation()}
		>
			<!-- Header -->
			<div class="flex items-center justify-between px-6 py-4 border-b border-gray-200">
				<div class="flex items-center gap-3">
					<div class="p-2 bg-blue-100 rounded-lg">
						<FileJson class="h-5 w-5 text-blue-600" />
					</div>
					<h2 class="text-lg font-semibold text-gray-900">Export as OpenAPI</h2>
				</div>
				<button
					onclick={onClose}
					disabled={isExporting}
					class="text-gray-400 hover:text-gray-600 transition-colors disabled:opacity-50"
				>
					<X class="h-5 w-5" />
				</button>
			</div>

			<!-- Schema Selection -->
			<div class="flex-1 overflow-y-auto px-6 py-4">
				{#if error}
					<div class="mb-4 p-3 bg-red-50 border border-red-200 rounded-lg text-red-700 text-sm">
						{error}
					</div>
				{/if}

				<!-- Select All -->
				<div class="mb-3 flex items-center justify-between">
					<button
						onclick={toggleAll}
						class="flex items-center gap-2 text-sm text-gray-600 hover:text-gray-900"
					>
						{#if allSelected}
							<CheckSquare class="h-4 w-4 text-blue-600" />
						{:else if someSelected}
							<CheckSquare class="h-4 w-4 text-blue-400" />
						{:else}
							<Square class="h-4 w-4" />
						{/if}
						<span>Select all ({schemas.length} schemas)</span>
					</button>
					<span class="text-sm text-gray-500">{selectedIds.size} selected</span>
				</div>

				<!-- Schema List -->
				<div class="space-y-1 mb-6 max-h-48 overflow-y-auto border rounded-lg p-2">
					{#each schemas as schema}
						<button
							onclick={() => toggleSchema(schema.id)}
							class="w-full flex items-center gap-3 p-2 rounded hover:bg-gray-50 transition-colors text-left"
						>
							{#if selectedIds.has(schema.id)}
								<CheckSquare class="h-4 w-4 text-blue-600 flex-shrink-0" />
							{:else}
								<Square class="h-4 w-4 text-gray-400 flex-shrink-0" />
							{/if}
							<Badge variant={methodColors[schema.httpMethod] || 'gray'} size="sm">
								{schema.httpMethod}
							</Badge>
							<code class="text-sm text-gray-900 flex-1 truncate">{schema.path}</code>
							<span class="text-xs text-gray-500">{schema.sampleCount} samples</span>
						</button>
					{/each}
				</div>

				<!-- Export Options -->
				<div class="space-y-4">
					<h3 class="text-sm font-medium text-gray-900">Export Options</h3>

					<div class="grid grid-cols-2 gap-4">
						<div>
							<label for="export-title" class="block text-sm font-medium text-gray-700 mb-1">
								Title <span class="text-red-500">*</span>
							</label>
							<input
								id="export-title"
								type="text"
								bind:value={title}
								placeholder="My API"
								class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500 text-sm"
							/>
						</div>
						<div>
							<label for="export-version" class="block text-sm font-medium text-gray-700 mb-1">
								Version <span class="text-red-500">*</span>
							</label>
							<input
								id="export-version"
								type="text"
								bind:value={version}
								placeholder="1.0.0"
								class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500 text-sm"
							/>
						</div>
					</div>

					<div>
						<label for="export-description" class="block text-sm font-medium text-gray-700 mb-1">
							Description
						</label>
						<textarea
							id="export-description"
							bind:value={description}
							placeholder="Optional description for this API..."
							rows="2"
							class="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:border-blue-500 text-sm"
						></textarea>
					</div>

					<label class="flex items-center gap-2">
						<input
							type="checkbox"
							bind:checked={includeMetadata}
							class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
						/>
						<span class="text-sm text-gray-700"
							>Include Flowplane metadata (x-flowplane-* extensions)</span
						>
					</label>
				</div>
			</div>

			<!-- Footer -->
			<div class="flex items-center justify-end gap-3 px-6 py-4 border-t border-gray-200">
				<Button onclick={onClose} variant="ghost" disabled={isExporting}>Cancel</Button>
				<Button onclick={handleExport} variant="primary" disabled={!canExport || isExporting}>
					{#if isExporting}
						<span class="flex items-center gap-2">
							<span
								class="animate-spin h-4 w-4 border-2 border-white border-t-transparent rounded-full"
							></span>
							Exporting...
						</span>
					{:else}
						<span class="flex items-center gap-2">
							<Download class="h-4 w-4" />
							Export {selectedIds.size} Schema{selectedIds.size !== 1 ? 's' : ''}
						</span>
					{/if}
				</Button>
			</div>
		</div>
	</div>
{/if}
