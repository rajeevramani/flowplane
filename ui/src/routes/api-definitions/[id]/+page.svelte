<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import type { ApiDefinitionSummary } from '$lib/api/types';

	let apiDefinition = $state<ApiDefinitionSummary | null>(null);
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let deleteConfirm = $state(false);
	let isDeleting = $state(false);

	// Get the ID from the URL
	const apiDefinitionId = $derived($page.params.id);

	onMount(async () => {
		// Check authentication
		try {
			await apiClient.getSessionInfo();
			await loadApiDefinition();
		} catch (err) {
			goto('/login');
		}
	});

	async function loadApiDefinition() {
		if (!apiDefinitionId) return;

		isLoading = true;
		error = null;

		try {
			apiDefinition = await apiClient.getApiDefinition(apiDefinitionId);
		} catch (err: any) {
			error = err.message || 'Failed to load API definition';
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
		if (!apiDefinition) return;

		try {
			isDeleting = true;
			await apiClient.deleteApiDefinition(apiDefinition.id);
			// Redirect to resources page after successful delete
			goto('/resources');
		} catch (err: any) {
			error = err.message || 'Failed to delete API definition';
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

<div class="min-h-screen bg-gray-50">
	<!-- Navigation -->
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a
						href="/resources"
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
					<h1 class="text-xl font-bold text-gray-900">API Definition Details</h1>
				</div>
				{#if apiDefinition}
					<button
						onclick={confirmDelete}
						class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700"
					>
						Delete
					</button>
				{/if}
			</div>
		</div>
	</nav>

	<main class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
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
		{:else if apiDefinition}
			<!-- Header Section -->
			<div class="bg-white rounded-lg shadow-md p-6 mb-6">
				<div class="flex justify-between items-start">
					<div>
						<h2 class="text-2xl font-bold text-gray-900">{apiDefinition.domain}</h2>
						<div class="mt-2 flex items-center gap-3">
							<span
								class="inline-block px-3 py-1 text-sm font-medium bg-blue-100 text-blue-800 rounded-full"
							>
								Team: {apiDefinition.team}
							</span>
							<span class="text-sm text-gray-600">Version: {apiDefinition.version}</span>
						</div>
					</div>
					<div class="text-right">
						<p class="text-sm text-gray-600">
							<span class="font-medium">ID:</span>
							{#if apiDefinition}
								<button
									onclick={() => copyToClipboard(apiDefinition?.id || '')}
									class="ml-1 text-blue-600 hover:text-blue-800 font-mono"
									title="Click to copy"
								>
									{apiDefinition.id}
								</button>
							{/if}
						</p>
					</div>
				</div>
			</div>

			<!-- Configuration Details -->
			<div class="grid grid-cols-1 lg:grid-cols-2 gap-6 mb-6">
				<!-- Basic Information -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<h3 class="text-lg font-semibold text-gray-900 mb-4">Basic Information</h3>
					<dl class="space-y-3">
						<div>
							<dt class="text-sm font-medium text-gray-500">Domain</dt>
							<dd class="mt-1 text-sm text-gray-900">{apiDefinition.domain}</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Team</dt>
							<dd class="mt-1 text-sm text-gray-900">{apiDefinition.team}</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Version</dt>
							<dd class="mt-1 text-sm text-gray-900">{apiDefinition.version}</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Created At</dt>
							<dd class="mt-1 text-sm text-gray-900">{formatDate(apiDefinition.createdAt)}</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Updated At</dt>
							<dd class="mt-1 text-sm text-gray-900">{formatDate(apiDefinition.updatedAt)}</dd>
						</div>
					</dl>
				</div>

				<!-- Listener Configuration -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<h3 class="text-lg font-semibold text-gray-900 mb-4">Listener Configuration</h3>
					<dl class="space-y-3">
						<div>
							<dt class="text-sm font-medium text-gray-500">Listener Isolation</dt>
							<dd class="mt-1">
								{#if apiDefinition.listenerIsolation}
									<span
										class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800"
									>
										Enabled
									</span>
									<p class="mt-1 text-sm text-gray-600">
										This API has a dedicated listener separate from other APIs.
									</p>
								{:else}
									<span
										class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-gray-100 text-gray-800"
									>
										Disabled
									</span>
									<p class="mt-1 text-sm text-gray-600">
										This API uses a shared listener with other APIs.
									</p>
								{/if}
							</dd>
						</div>
					</dl>
				</div>
			</div>

			<!-- Envoy Configuration -->
			{#if apiDefinition.bootstrapUri}
				<div class="bg-white rounded-lg shadow-md p-6 mb-6">
					<h3 class="text-lg font-semibold text-gray-900 mb-4">Envoy Configuration</h3>
					<div class="space-y-4">
						<div>
							<dt class="text-sm font-medium text-gray-500 mb-2">Envoy Bootstrap URI</dt>
							<dd class="flex items-center gap-2">
								{#if apiDefinition.bootstrapUri}
									<code class="flex-1 px-3 py-2 bg-gray-50 border border-gray-200 rounded text-sm font-mono text-gray-900">
										{apiDefinition.bootstrapUri}
									</code>
									<button
										onclick={() => copyToClipboard(apiDefinition?.bootstrapUri || '')}
										class="px-3 py-2 text-sm font-medium text-blue-600 hover:text-blue-800 border border-blue-300 rounded hover:bg-blue-50"
										title="Copy to clipboard"
									>
										Copy
									</button>
									<a
										href="/generate-envoy-config"
										class="px-3 py-2 text-sm font-medium text-white bg-blue-600 rounded hover:bg-blue-700"
									>
										Generate
									</a>
								{/if}
							</dd>
						</div>
						<div class="bg-blue-50 border-l-4 border-blue-500 p-4">
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
										Use this URI to configure your Envoy proxy instances. The bootstrap
										configuration will automatically include all routes, clusters, and listeners for
										this API definition.
									</p>
								</div>
							</div>
						</div>
					</div>
				</div>
			{/if}

			<!-- Quick Actions -->
			<div class="bg-white rounded-lg shadow-md p-6">
				<h3 class="text-lg font-semibold text-gray-900 mb-4">Quick Actions</h3>
				<div class="flex flex-wrap gap-3">
					<a
						href="/resources?tab=api-definitions"
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200"
					>
						View All API Definitions
					</a>
					<a
						href="/resources?tab=routes"
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200"
					>
						View Routes
					</a>
					<a
						href="/resources?tab=listeners"
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200"
					>
						View Listeners
					</a>
					<a
						href="/resources?tab=clusters"
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200"
					>
						View Clusters
					</a>
				</div>
			</div>
		{/if}
	</main>
</div>

<!-- Delete Confirmation Modal -->
{#if deleteConfirm && apiDefinition}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
	>
		<div class="bg-white rounded-lg shadow-xl p-6 max-w-md w-full">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Confirm Delete</h2>
			<p class="text-sm text-gray-600 mb-6">
				Are you sure you want to delete the API definition for
				<strong class="text-gray-900">{apiDefinition.domain}</strong>?
				This action cannot be undone and will remove all associated routes and configurations.
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
