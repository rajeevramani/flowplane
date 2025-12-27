<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Plus, Edit, Trash2, Download, Eye, Puzzle, HardDrive, Database } from 'lucide-svelte';
	import type { CustomWasmFilterResponse, FilterResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import DeleteConfirmModal from '$lib/components/DeleteConfirmModal.svelte';

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);
	let searchQuery = $state('');
	let currentTeam = $state<string>('');

	// Data
	let customFilters = $state<CustomWasmFilterResponse[]>([]);
	let filterInstances = $state<FilterResponse[]>([]);

	// Delete modal state
	let showDeleteModal = $state(false);
	let filterToDelete = $state<CustomWasmFilterResponse | null>(null);
	let isDeleting = $state(false);

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		if (currentTeam && currentTeam !== value) {
			currentTeam = value;
			loadData();
		} else {
			currentTeam = value;
		}
	});

	onMount(async () => {
		await loadData();
	});

	async function loadData() {
		if (!currentTeam) return;

		isLoading = true;
		error = null;

		try {
			// Load custom WASM filters
			const response = await apiClient.listCustomWasmFilters(currentTeam);
			customFilters = response.items;

			// Load filter instances to count usage
			const filters = await apiClient.listFilters();
			filterInstances = filters;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load custom filters';
			console.error('Failed to load custom filters:', e);
		} finally {
			isLoading = false;
		}
	}

	// Calculate stats
	let stats = $derived(() => {
		const totalFilters = customFilters.length;
		const totalSize = customFilters.reduce((sum, f) => sum + (f.wasm_size_bytes || 0), 0);

		// Count filter instances using custom filter types
		let inUseCount = 0;
		customFilters.forEach((cf) => {
			const instancesUsingThis = filterInstances.filter(
				(fi) => fi.filterType === cf.filter_type
			).length;
			if (instancesUsingThis > 0) inUseCount++;
		});

		return {
			totalFilters,
			inUseCount,
			totalSize
		};
	});

	// Get usage count for a custom filter
	function getUsageCount(customFilter: CustomWasmFilterResponse): number {
		return filterInstances.filter((fi) => fi.filterType === customFilter.filter_type).length;
	}

	// Filter by search
	let filteredFilters = $derived(
		customFilters.filter(
			(filter) =>
				!searchQuery ||
				filter.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				filter.display_name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				(filter.description && filter.description.toLowerCase().includes(searchQuery.toLowerCase()))
		)
	);

	// Format bytes to human readable
	function formatBytes(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	// Format date
	function formatDate(dateStr: string): string {
		const date = new Date(dateStr);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}

	// Navigate to upload page
	function handleUpload() {
		goto('/custom-filters/upload');
	}

	// Navigate to view page
	function handleView(id: string) {
		goto(`/custom-filters/${encodeURIComponent(id)}`);
	}

	// Navigate to edit page
	function handleEdit(id: string) {
		goto(`/custom-filters/${encodeURIComponent(id)}/edit`);
	}

	// Download WASM binary
	async function handleDownload(filter: CustomWasmFilterResponse) {
		try {
			const blob = await apiClient.downloadCustomWasmFilterBinary(currentTeam, filter.id);
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `${filter.name}.wasm`;
			document.body.appendChild(a);
			a.click();
			document.body.removeChild(a);
			URL.revokeObjectURL(url);
		} catch (e) {
			actionError = e instanceof Error ? e.message : 'Failed to download WASM binary';
		}
	}

	// Open delete confirmation modal
	function openDeleteModal(filter: CustomWasmFilterResponse) {
		filterToDelete = filter;
		showDeleteModal = true;
	}

	// Confirm delete
	async function confirmDelete() {
		if (!filterToDelete) return;

		isDeleting = true;
		actionError = null;

		try {
			await apiClient.deleteCustomWasmFilter(currentTeam, filterToDelete.id);
			showDeleteModal = false;
			filterToDelete = null;
			await loadData();
		} catch (e) {
			actionError = e instanceof Error ? e.message : 'Failed to delete custom filter';
		} finally {
			isDeleting = false;
		}
	}

	// Cancel delete
	function cancelDelete() {
		showDeleteModal = false;
		filterToDelete = null;
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Custom Filters</h1>
		<p class="mt-2 text-sm text-gray-600">
			Manage custom WebAssembly filters for the <span class="font-medium">{currentTeam}</span> team
		</p>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6">
		<Button onclick={handleUpload} variant="primary">
			<Plus class="h-4 w-4 mr-2" />
			Upload Custom Filter
		</Button>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-3 gap-4 mb-6">
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Custom Filters</p>
					<p class="text-2xl font-bold text-gray-900">{stats().totalFilters}</p>
				</div>
				<div class="p-3 bg-purple-100 rounded-lg">
					<Puzzle class="h-6 w-6 text-purple-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">In Use</p>
					<p class="text-2xl font-bold text-gray-900">{stats().inUseCount}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<Database class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Size</p>
					<p class="text-2xl font-bold text-gray-900">{formatBytes(stats().totalSize)}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<HardDrive class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>
	</div>

	<!-- Search -->
	<div class="mb-6">
		<input
			type="text"
			bind:value={searchQuery}
			placeholder="Search by name, display name, or description..."
			class="w-full md:w-96 px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		/>
	</div>

	<!-- Action Error -->
	{#if actionError}
		<div class="bg-red-50 border border-red-200 rounded-md p-4 mb-6">
			<p class="text-sm text-red-800">{actionError}</p>
		</div>
	{/if}

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading custom filters...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filteredFilters.length === 0}
		<!-- Empty State -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
			<Puzzle class="h-12 w-12 text-gray-400 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">
				{searchQuery ? 'No custom filters found' : 'No custom filters yet'}
			</h3>
			<p class="text-sm text-gray-600 mb-6">
				{searchQuery
					? 'Try adjusting your search query'
					: 'Upload a WASM binary to create your first custom filter'}
			</p>
			{#if !searchQuery}
				<Button onclick={handleUpload} variant="primary">
					<Plus class="h-4 w-4 mr-2" />
					Upload Custom Filter
				</Button>
			{/if}
		</div>
	{:else}
		<!-- Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Name
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Display Name
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Size
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Instances
						</th>
						<th
							class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Created
						</th>
						<th
							class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
						>
							Actions
						</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each filteredFilters as filter}
						{@const usageCount = getUsageCount(filter)}
						<tr class="hover:bg-gray-50 transition-colors">
							<!-- Name -->
							<td class="px-6 py-4">
								<div class="flex flex-col">
									<span class="text-sm font-medium text-gray-900">{filter.name}</span>
									<span class="text-xs text-gray-500 font-mono">{filter.filter_type}</span>
								</div>
							</td>

							<!-- Display Name -->
							<td class="px-6 py-4">
								<div class="flex flex-col">
									<span class="text-sm text-gray-900">{filter.display_name}</span>
									<span class="text-xs text-gray-500">{filter.description || '-'}</span>
								</div>
							</td>

							<!-- Size -->
							<td class="px-6 py-4">
								<Badge variant="gray">{formatBytes(filter.wasm_size_bytes)}</Badge>
							</td>

							<!-- Instances -->
							<td class="px-6 py-4">
								{#if usageCount > 0}
									<span
										class="inline-flex items-center px-2 py-0.5 text-xs rounded bg-green-100 text-green-700"
									>
										{usageCount} {usageCount === 1 ? 'instance' : 'instances'}
									</span>
								{:else}
									<span class="px-2 py-0.5 text-xs rounded bg-gray-100 text-gray-500">
										Not used
									</span>
								{/if}
							</td>

							<!-- Created -->
							<td class="px-6 py-4">
								<span class="text-sm text-gray-600">{formatDate(filter.created_at)}</span>
							</td>

							<!-- Actions -->
							<td class="px-6 py-4 text-right">
								<div class="flex justify-end gap-1">
									<button
										onclick={() => handleView(filter.id)}
										class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
										title="View details"
									>
										<Eye class="h-4 w-4" />
									</button>
									<button
										onclick={() => handleEdit(filter.id)}
										class="p-2 text-orange-600 hover:bg-orange-50 rounded-md transition-colors"
										title="Edit metadata"
									>
										<Edit class="h-4 w-4" />
									</button>
									<button
										onclick={() => handleDownload(filter)}
										class="p-2 text-green-600 hover:bg-green-50 rounded-md transition-colors"
										title="Download WASM binary"
									>
										<Download class="h-4 w-4" />
									</button>
									<button
										onclick={() => openDeleteModal(filter)}
										class="p-2 text-red-600 hover:bg-red-50 rounded-md transition-colors"
										title="Delete custom filter"
									>
										<Trash2 class="h-4 w-4" />
									</button>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		<!-- Pagination Placeholder -->
		{#if filteredFilters.length > 50}
			<div class="mt-4 flex justify-center">
				<p class="text-sm text-gray-600">Showing {filteredFilters.length} custom filters</p>
			</div>
		{/if}
	{/if}
</div>

<!-- Delete Confirmation Modal -->
<DeleteConfirmModal
	show={showDeleteModal && filterToDelete !== null}
	resourceType="custom filter"
	resourceName={filterToDelete?.display_name ?? ''}
	onConfirm={confirmDelete}
	onCancel={cancelDelete}
	loading={isDeleting}
	warningMessage={filterToDelete && getUsageCount(filterToDelete) > 0
		? `Warning: This filter is used by ${getUsageCount(filterToDelete)} filter instance(s). Deleting may break those configurations.`
		: undefined}
/>
