<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import Badge from '$lib/components/Badge.svelte';
	import type { ImportDetailsResponse } from '$lib/api/types';

	let importDetails = $state<ImportDetailsResponse | null>(null);
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let deleteConfirm = $state(false);
	let isDeleting = $state(false);

	// Get the ID from the URL
	const importId = $derived($page.params.id);

	onMount(async () => {
		await loadImport();
	});

	async function loadImport() {
		if (!importId) return;

		isLoading = true;
		error = null;

		try {
			importDetails = await apiClient.getImport(importId);
		} catch (err: any) {
			error = err.message || 'Failed to load import details';
		} finally {
			isLoading = false;
		}
	}

	function confirmDelete() {
		deleteConfirm = true;
	}

	function cancelDelete() {
		deleteConfirm = false;
	}

	async function handleDelete() {
		if (!importDetails) return;

		try {
			isDeleting = true;
			await apiClient.deleteImport(importDetails.id);
			// Redirect to resources page after successful delete
			goto('/resources?tab=imports');
		} catch (err: any) {
			error = err.message || 'Failed to delete import';
			deleteConfirm = false;
		} finally {
			isDeleting = false;
		}
	}

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleString('en-US', {
			year: 'numeric',
			month: 'long',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function copyToClipboard(text: string) {
		navigator.clipboard.writeText(text);
	}
</script>

<!-- Page Header with Actions -->
<div class="flex justify-between items-center mb-6">
	<div class="flex items-center gap-4">
		<a
			href="/resources?tab=imports"
			class="text-blue-600 hover:text-blue-800"
			aria-label="Back to resources"
		>
			<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					stroke-width="2"
					d="M10 19l-7-7m0 0l7-7m-7 7h18"
				/>
			</svg>
		</a>
		<h1 class="text-2xl font-bold text-gray-900">Import Details</h1>
	</div>
	{#if importDetails}
		<button
			onclick={confirmDelete}
			class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700"
		>
			Delete
		</button>
	{/if}
</div>

<!-- Error Message -->
{#if error}
	<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
		<p class="text-red-800 text-sm">{error}</p>
	</div>
{/if}

{#if isLoading}
	<div class="flex justify-center items-center py-12">
		<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
	</div>
{:else if importDetails}
	<!-- Header Section -->
	<div class="bg-white rounded-lg shadow-md p-6 mb-6">
		<div class="flex justify-between items-start">
			<div>
				<h2 class="text-2xl font-bold text-gray-900">{importDetails.specName}</h2>
				<div class="mt-2 flex items-center gap-3">
					<Badge variant="blue">Team: {importDetails.team}</Badge>
					{#if importDetails.specVersion}
						<span class="text-sm text-gray-600">Version {importDetails.specVersion}</span>
					{/if}
				</div>
			</div>
			<div class="text-right">
				<p class="text-sm text-gray-600">
					<span class="font-medium">ID:</span>
					<button
						onclick={() => copyToClipboard(importDetails?.id || '')}
						class="ml-1 text-blue-600 hover:text-blue-800 font-mono text-xs"
						title="Click to copy"
					>
						{importDetails.id}
					</button>
				</p>
			</div>
		</div>
	</div>

	<!-- Import Details -->
	<div class="grid grid-cols-1 lg:grid-cols-2 gap-6 mb-6">
		<!-- Basic Information -->
		<div class="bg-white rounded-lg shadow-md p-6">
			<h3 class="text-lg font-semibold text-gray-900 mb-4">Import Information</h3>
			<dl class="space-y-3">
				<div>
					<dt class="text-sm font-medium text-gray-500">Spec Name</dt>
					<dd class="mt-1 text-sm text-gray-900">{importDetails.specName}</dd>
				</div>
				{#if importDetails.specVersion}
					<div>
						<dt class="text-sm font-medium text-gray-500">Spec Version</dt>
						<dd class="mt-1 text-sm text-gray-900">{importDetails.specVersion}</dd>
					</div>
				{/if}
				{#if importDetails.specChecksum}
					<div>
						<dt class="text-sm font-medium text-gray-500">Checksum</dt>
						<dd class="mt-1 text-sm text-gray-900 font-mono text-xs break-all">{importDetails.specChecksum}</dd>
					</div>
				{/if}
				<div>
					<dt class="text-sm font-medium text-gray-500">Team</dt>
					<dd class="mt-1 text-sm text-gray-900">{importDetails.team}</dd>
				</div>
				<div>
					<dt class="text-sm font-medium text-gray-500">Imported At</dt>
					<dd class="mt-1 text-sm text-gray-900">{formatDate(importDetails.importedAt)}</dd>
				</div>
				<div>
					<dt class="text-sm font-medium text-gray-500">Updated At</dt>
					<dd class="mt-1 text-sm text-gray-900">{formatDate(importDetails.updatedAt)}</dd>
				</div>
			</dl>
		</div>

		<!-- Resource Counts -->
		<div class="bg-white rounded-lg shadow-md p-6">
			<h3 class="text-lg font-semibold text-gray-900 mb-4">Resources Created</h3>
			<dl class="space-y-3">
				<div>
					<dt class="text-sm font-medium text-gray-500">Routes</dt>
					<dd class="mt-1 text-2xl font-bold text-green-600">{importDetails.routeCount}</dd>
				</div>
				<div>
					<dt class="text-sm font-medium text-gray-500">Listeners</dt>
					<dd class="mt-1 text-2xl font-bold text-blue-600">{importDetails.listenerCount}</dd>
				</div>
				<div>
					<dt class="text-sm font-medium text-gray-500">Clusters</dt>
					<dd class="mt-1 text-2xl font-bold text-purple-600">{importDetails.clusterCount}</dd>
				</div>
			</dl>
		</div>
	</div>

	<!-- Info Message -->
	<div class="bg-blue-50 border-l-4 border-blue-500 rounded-md p-4 mb-6">
		<div class="flex">
			<div class="flex-shrink-0">
				<svg
					class="h-5 w-5 text-blue-400"
					fill="currentColor"
					viewBox="0 0 20 20"
				>
					<path
						fill-rule="evenodd"
						d="M18 10a8 8 0 11-16 0 8 8 0 0116 0zm-7-4a1 1 0 11-2 0 1 1 0 012 0zM9 9a1 1 0 000 2v3a1 1 0 001 1h1a1 1 0 100-2v-3a1 1 0 00-1-1H9z"
						clip-rule="evenodd"
					/>
				</svg>
			</div>
			<div class="ml-3">
				<p class="text-sm text-blue-700">
					This import created {importDetails.routeCount} route{importDetails.routeCount !== 1 ? 's' : ''}, {importDetails.clusterCount} cluster{importDetails.clusterCount !== 1 ? 's' : ''}, and {importDetails.listenerCount} listener{importDetails.listenerCount !== 1 ? 's' : ''}.
					You can view these resources in the <a href="/resources" class="font-medium underline">Resources</a> page.
				</p>
			</div>
		</div>
	</div>

	<!-- Quick Actions -->
	<div class="bg-white rounded-lg shadow-md p-6">
		<h3 class="text-lg font-semibold text-gray-900 mb-4">Quick Actions</h3>
		<div class="flex flex-wrap gap-3">
			<a
				href="/resources?tab=imports"
				class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200"
			>
				← Back to Imports
			</a>
			<a
				href="/resources?tab=routes"
				class="px-4 py-2 text-sm font-medium text-white bg-green-600 rounded-md hover:bg-green-700"
			>
				View Routes
			</a>
			<a
				href="/resources?tab=clusters"
				class="px-4 py-2 text-sm font-medium text-white bg-purple-600 rounded-md hover:bg-purple-700"
			>
				View Clusters
			</a>
			<a
				href="/generate-envoy-config"
				class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
			>
				Generate Envoy Config
			</a>
		</div>
	</div>
{/if}

<!-- Delete Confirmation Modal -->
{#if deleteConfirm && importDetails}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
	>
		<div class="bg-white rounded-lg shadow-xl p-6 max-w-md w-full">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Confirm Delete</h2>
			<p class="text-sm text-gray-600 mb-6">
				Are you sure you want to delete the import for
				<strong class="text-gray-900">{importDetails.specName}</strong>?
				This will also delete all {importDetails.routeCount} route{importDetails.routeCount !== 1 ? 's' : ''} and {importDetails.clusterCount} cluster{importDetails.clusterCount !== 1 ? 's' : ''} created by this import.
				This action cannot be undone.
			</p>
			<div class="flex justify-end gap-3">
				<button
					onclick={cancelDelete}
					disabled={isDeleting}
					class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200 disabled:opacity-50"
				>
					Cancel
				</button>
				<button
					onclick={handleDelete}
					disabled={isDeleting}
					class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700 disabled:opacity-50"
				>
					{isDeleting ? 'Deleting...' : 'Delete'}
				</button>
			</div>
		</div>
	</div>
{/if}
