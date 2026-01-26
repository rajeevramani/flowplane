<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { ArrowLeft, Save, Server, Download, Copy, Check } from 'lucide-svelte';
	import type { DataplaneResponse } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import { validateRequired, validateIdentifier, runValidators } from '$lib/utils/validators';
	import { selectedTeam } from '$lib/stores/team';

	// Get dataplane name from URL (the [id] param is actually the name)
	let dataplaneName = $derived($page.params.id);

	// Get current team from store
	let currentTeam = $state<string>('');
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Form state
	let formState = $state({
		name: '',
		gatewayHost: '',
		description: ''
	});

	let originalDataplane = $state<DataplaneResponse | null>(null);
	let isLoading = $state(true);
	let isSubmitting = $state(false);
	let error = $state<string | null>(null);
	let bootstrapConfig = $state<string | null>(null);
	let showBootstrap = $state(false);
	let copied = $state(false);

	// Load dataplane on mount
	onMount(async () => {
		await loadDataplane();
	});

	async function loadDataplane() {
		if (!dataplaneName || !currentTeam) {
			error = 'No dataplane name or team provided';
			return;
		}

		isLoading = true;
		error = null;

		try {
			const dataplane = await apiClient.getDataplane(currentTeam, dataplaneName);
			originalDataplane = dataplane;

			formState = {
				name: dataplane.name,
				gatewayHost: dataplane.gatewayHost || '',
				description: dataplane.description || ''
			};
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load dataplane';
		} finally {
			isLoading = false;
		}
	}

	// Validation
	function validateForm(): string | null {
		return runValidators([
			() => validateRequired(formState.name, 'Dataplane name'),
			() => validateIdentifier(formState.name, 'Dataplane name')
		]);
	}

	// Handle form submission
	async function handleSubmit(e: Event) {
		e.preventDefault();

		if (!dataplaneName || !currentTeam) {
			error = 'No dataplane name or team provided';
			return;
		}

		const validationError = validateForm();
		if (validationError) {
			error = validationError;
			return;
		}

		isSubmitting = true;
		error = null;

		try {
			await apiClient.updateDataplane(currentTeam, dataplaneName, {
				gatewayHost: formState.gatewayHost || undefined,
				description: formState.description || undefined
			});

			goto('/dataplanes');
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to update dataplane';
		} finally {
			isSubmitting = false;
		}
	}

	// Navigate back
	function handleBack() {
		goto('/dataplanes');
	}

	// Download bootstrap
	async function handleDownloadBootstrap() {
		if (!dataplaneName || !currentTeam) return;

		try {
			const bootstrap = await apiClient.getDataplaneBootstrap(currentTeam, dataplaneName, 'yaml');
			const blob = new Blob([bootstrap], { type: 'application/yaml' });
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `envoy-bootstrap-${formState.name}.yaml`;
			document.body.appendChild(a);
			a.click();
			document.body.removeChild(a);
			URL.revokeObjectURL(url);
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to download bootstrap';
		}
	}

	// Show/hide bootstrap preview
	async function toggleBootstrapPreview() {
		if (!dataplaneName || !currentTeam) return;

		if (!showBootstrap && !bootstrapConfig) {
			try {
				bootstrapConfig = await apiClient.getDataplaneBootstrap(currentTeam, dataplaneName, 'yaml');
			} catch (err) {
				error = err instanceof Error ? err.message : 'Failed to load bootstrap';
				return;
			}
		}
		showBootstrap = !showBootstrap;
	}

	// Copy bootstrap to clipboard
	async function handleCopyBootstrap() {
		if (bootstrapConfig) {
			try {
				await navigator.clipboard.writeText(bootstrapConfig);
				copied = true;
				setTimeout(() => {
					copied = false;
				}, 2000);
			} catch (err) {
				console.error('Failed to copy:', err);
			}
		}
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8 max-w-4xl mx-auto">
	<!-- Header -->
	<div class="mb-8">
		<button
			onclick={handleBack}
			class="flex items-center text-sm text-gray-600 hover:text-gray-900 mb-4"
		>
			<ArrowLeft class="h-4 w-4 mr-1" />
			Back to Dataplanes
		</button>

		<div class="flex items-center justify-between">
			<div class="flex items-center gap-3">
				<div class="p-3 bg-blue-100 rounded-lg">
					<Server class="h-6 w-6 text-blue-600" />
				</div>
				<div>
					<h1 class="text-2xl font-bold text-gray-900">Edit Dataplane</h1>
					<p class="text-sm text-gray-600">
						{#if originalDataplane}
							Editing <span class="font-medium">{originalDataplane.name}</span>
						{:else}
							Loading...
						{/if}
					</p>
				</div>
			</div>

			{#if originalDataplane}
				<Button onclick={handleDownloadBootstrap} variant="secondary">
					<Download class="h-4 w-4 mr-2" />
					Download Bootstrap
				</Button>
			{/if}
		</div>
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading dataplane...</span>
			</div>
		</div>
	{:else if error && !originalDataplane}
		<!-- Error State (failed to load) -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else}
		<!-- Error Alert (form errors) -->
		{#if error}
			<div class="bg-red-50 border border-red-200 rounded-md p-4 mb-6">
				<p class="text-sm text-red-800">{error}</p>
			</div>
		{/if}

		<!-- Form -->
		<form onsubmit={handleSubmit} class="space-y-6">
			<!-- Basic Information -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<h2 class="text-lg font-medium text-gray-900 mb-4">Basic Information</h2>

				<div class="space-y-4">
					<!-- Name -->
					<div>
						<label for="name" class="block text-sm font-medium text-gray-700 mb-1">
							Name <span class="text-red-500">*</span>
						</label>
						<input
							type="text"
							id="name"
							bind:value={formState.name}
							placeholder="e.g., prod-gateway, staging-envoy"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							required
						/>
						<p class="mt-1 text-xs text-gray-500">
							A unique identifier for this dataplane. Use lowercase letters, numbers, and hyphens.
						</p>
					</div>

					<!-- Description -->
					<div>
						<label for="description" class="block text-sm font-medium text-gray-700 mb-1">
							Description
						</label>
						<textarea
							id="description"
							bind:value={formState.description}
							placeholder="Optional description for this dataplane"
							rows="2"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						></textarea>
					</div>
				</div>
			</div>

			<!-- Network Configuration -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<h2 class="text-lg font-medium text-gray-900 mb-4">Network Configuration</h2>

				<div class="space-y-4">
					<!-- Gateway Host -->
					<div>
						<label for="gatewayHost" class="block text-sm font-medium text-gray-700 mb-1">
							Gateway Host
						</label>
						<input
							type="text"
							id="gatewayHost"
							bind:value={formState.gatewayHost}
							placeholder="e.g., 10.0.0.5, host.docker.internal, envoy.service.consul"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<p class="mt-1 text-xs text-gray-500">
							The address where this Envoy instance is reachable from the Control Plane. Leave
							empty to use localhost (127.0.0.1).
						</p>
					</div>
				</div>
			</div>

			<!-- Bootstrap Preview -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<div class="flex items-center justify-between mb-4">
					<h2 class="text-lg font-medium text-gray-900">Bootstrap Configuration</h2>
					<button
						type="button"
						onclick={toggleBootstrapPreview}
						class="text-sm text-blue-600 hover:text-blue-800"
					>
						{showBootstrap ? 'Hide' : 'Show'} Bootstrap YAML
					</button>
				</div>

				{#if showBootstrap && bootstrapConfig}
					<div class="relative">
						<button
							type="button"
							onclick={handleCopyBootstrap}
							class="absolute top-2 right-2 p-2 text-gray-400 hover:text-gray-600 bg-gray-800 rounded"
							title="Copy to clipboard"
						>
							{#if copied}
								<Check class="h-4 w-4 text-green-400" />
							{:else}
								<Copy class="h-4 w-4" />
							{/if}
						</button>
						<pre
							class="bg-gray-900 text-gray-100 p-4 rounded-md text-sm overflow-x-auto max-h-96">{bootstrapConfig}</pre>
					</div>
					<p class="mt-2 text-xs text-gray-500">
						Use this configuration to start your Envoy instance with the correct node ID and xDS
						connection settings.
					</p>
				{:else}
					<p class="text-sm text-gray-600">
						Click "Show Bootstrap YAML" to preview the Envoy bootstrap configuration for this
						dataplane.
					</p>
				{/if}
			</div>

			<!-- Form Actions -->
			<div class="flex justify-end gap-3">
				<Button onclick={handleBack} variant="secondary" disabled={isSubmitting}>
					Cancel
				</Button>
				<Button type="submit" variant="primary" disabled={isSubmitting}>
					{#if isSubmitting}
						<div class="animate-spin rounded-full h-4 w-4 border-b-2 border-white mr-2"></div>
						Saving...
					{:else}
						<Save class="h-4 w-4 mr-2" />
						Save Changes
					{/if}
				</Button>
			</div>
		</form>
	{/if}
</div>
