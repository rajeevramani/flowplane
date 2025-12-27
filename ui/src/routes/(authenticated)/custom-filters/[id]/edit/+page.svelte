<script lang="ts">
	import { page } from '$app/stores';
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import {
		ArrowLeft,
		Save,
		AlertTriangle,
		ChevronDown,
		ChevronUp,
		HardDrive,
		Download
	} from 'lucide-svelte';
	import type { CustomWasmFilterResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	let currentTeam = $state<string>('');
	let filterId = $derived($page.params.id);

	// Original data
	let customFilter = $state<CustomWasmFilterResponse | null>(null);
	let isLoading = $state(true);
	let loadError = $state<string | null>(null);

	// Form fields
	let displayName = $state('');
	let description = $state('');
	let configSchemaText = $state('');
	let perRouteSchemaText = $state('');
	let uiHintsText = $state('');

	// Advanced options
	let showAdvanced = $state(false);
	let attachmentPoints = $state<string[]>([]);

	// Validation state
	let configSchemaValid = $state<boolean | null>(null);
	let configSchemaError = $state<string | null>(null);
	let perRouteSchemaValid = $state<boolean | null>(null);
	let perRouteSchemaError = $state<string | null>(null);
	let uiHintsValid = $state<boolean | null>(null);
	let uiHintsError = $state<string | null>(null);

	// Submission state
	let isSubmitting = $state(false);
	let submitError = $state<string | null>(null);

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
		if (!currentTeam || !filterId) return;

		isLoading = true;
		loadError = null;

		try {
			customFilter = await apiClient.getCustomWasmFilter(currentTeam, filterId);

			// Populate form fields
			displayName = customFilter.display_name;
			description = customFilter.description || '';
			configSchemaText = JSON.stringify(customFilter.config_schema, null, 2);
			perRouteSchemaText = customFilter.per_route_config_schema
				? JSON.stringify(customFilter.per_route_config_schema, null, 2)
				: '';
			uiHintsText = customFilter.ui_hints
				? JSON.stringify(customFilter.ui_hints, null, 2)
				: '';
			attachmentPoints = [...(customFilter.attachment_points || [])];

			// Initial validation
			handleConfigSchemaChange();
			handlePerRouteSchemaChange();
			handleUiHintsChange();
		} catch (e) {
			loadError = e instanceof Error ? e.message : 'Failed to load custom filter';
			console.error('Failed to load custom filter:', e);
		} finally {
			isLoading = false;
		}
	}

	// Format bytes
	function formatBytes(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	// Validate JSON
	function validateJson(text: string): { valid: boolean; error: string | null; parsed: unknown } {
		if (!text.trim()) {
			return { valid: true, error: null, parsed: null };
		}
		try {
			const parsed = JSON.parse(text);
			return { valid: true, error: null, parsed };
		} catch (e) {
			return { valid: false, error: e instanceof Error ? e.message : 'Invalid JSON', parsed: null };
		}
	}

	// Validate config schema on change
	function handleConfigSchemaChange() {
		const result = validateJson(configSchemaText);
		configSchemaValid = result.valid;
		configSchemaError = result.error;
	}

	// Validate per-route schema on change
	function handlePerRouteSchemaChange() {
		if (!perRouteSchemaText.trim()) {
			perRouteSchemaValid = null;
			perRouteSchemaError = null;
			return;
		}
		const result = validateJson(perRouteSchemaText);
		perRouteSchemaValid = result.valid;
		perRouteSchemaError = result.error;
	}

	// Validate UI hints on change
	function handleUiHintsChange() {
		if (!uiHintsText.trim()) {
			uiHintsValid = null;
			uiHintsError = null;
			return;
		}
		const result = validateJson(uiHintsText);
		uiHintsValid = result.valid;
		uiHintsError = result.error;
	}

	// Toggle attachment point
	function toggleAttachmentPoint(point: string) {
		if (attachmentPoints.includes(point)) {
			attachmentPoints = attachmentPoints.filter((p) => p !== point);
		} else {
			attachmentPoints = [...attachmentPoints, point];
		}
	}

	// Check if form is valid
	let isFormValid = $derived(
		displayName.trim().length > 0 &&
		configSchemaValid === true &&
		(perRouteSchemaValid === null || perRouteSchemaValid === true) &&
		(uiHintsValid === null || uiHintsValid === true) &&
		attachmentPoints.length > 0
	);

	// Check if form has changes
	let hasChanges = $derived(() => {
		if (!customFilter) return false;

		const originalConfigSchema = JSON.stringify(customFilter.config_schema, null, 2);
		const originalPerRouteSchema = customFilter.per_route_config_schema
			? JSON.stringify(customFilter.per_route_config_schema, null, 2)
			: '';
		const originalUiHints = customFilter.ui_hints
			? JSON.stringify(customFilter.ui_hints, null, 2)
			: '';

		// Create copies before sorting to avoid mutating state
		const sortedAttachmentPoints = [...attachmentPoints].sort();
		const sortedOriginalPoints = [...(customFilter.attachment_points || [])].sort();

		return (
			displayName !== customFilter.display_name ||
			description !== (customFilter.description || '') ||
			configSchemaText !== originalConfigSchema ||
			perRouteSchemaText !== originalPerRouteSchema ||
			uiHintsText !== originalUiHints ||
			JSON.stringify(sortedAttachmentPoints) !== JSON.stringify(sortedOriginalPoints)
		);
	});

	// Handle form submission
	async function handleSubmit() {
		if (!isFormValid || !currentTeam || !filterId) return;

		isSubmitting = true;
		submitError = null;

		try {
			const request = {
				display_name: displayName.trim(),
				description: description.trim() || undefined,
				config_schema: JSON.parse(configSchemaText),
				per_route_config_schema: perRouteSchemaText.trim()
					? JSON.parse(perRouteSchemaText)
					: undefined,
				ui_hints: uiHintsText.trim() ? JSON.parse(uiHintsText) : undefined,
				attachment_points: attachmentPoints
			};

			await apiClient.updateCustomWasmFilter(currentTeam, filterId, request);
			goto(`/custom-filters/${filterId}`);
		} catch (e) {
			submitError = e instanceof Error ? e.message : 'Failed to update custom filter';
		} finally {
			isSubmitting = false;
		}
	}

	// Download binary
	async function handleDownload() {
		if (!customFilter) return;

		try {
			const blob = await apiClient.downloadCustomWasmFilterBinary(currentTeam, customFilter.id);
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `${customFilter.name}.wasm`;
			document.body.appendChild(a);
			a.click();
			document.body.removeChild(a);
			URL.revokeObjectURL(url);
		} catch (e) {
			console.error('Failed to download:', e);
		}
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="flex items-center gap-4 mb-6">
		<a
			href={`/custom-filters/${filterId}`}
			class="text-blue-600 hover:text-blue-800"
			aria-label="Back to custom filter"
		>
			<ArrowLeft class="h-6 w-6" />
		</a>
		<div>
			<h1 class="text-2xl font-bold text-gray-900">Edit Custom Filter</h1>
			{#if customFilter}
				<p class="text-sm text-gray-500">{customFilter.name}</p>
			{/if}
		</div>
	</div>

	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading custom filter...</span>
			</div>
		</div>
	{:else if loadError}
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<div class="flex items-center gap-2">
				<AlertTriangle class="h-5 w-5 text-red-500" />
				<p class="text-sm text-red-800">{loadError}</p>
			</div>
		</div>
	{:else if customFilter}
		<div class="grid grid-cols-1 lg:grid-cols-2 gap-8">
			<!-- Left Column: Form -->
			<div class="space-y-6">
				<!-- Binary Info (Read-Only) -->
				<div class="bg-gray-50 rounded-lg border border-gray-200 p-6">
					<div class="flex items-center justify-between mb-3">
						<div class="flex items-center gap-2">
							<HardDrive class="h-5 w-5 text-gray-500" />
							<h2 class="text-lg font-semibold text-gray-900">WASM Binary</h2>
						</div>
						<button
							onclick={handleDownload}
							class="flex items-center gap-1 text-sm text-blue-600 hover:text-blue-800"
						>
							<Download class="h-4 w-4" />
							Download
						</button>
					</div>
					<div class="grid grid-cols-2 gap-4 text-sm">
						<div>
							<p class="text-gray-500">Size</p>
							<p class="font-medium text-gray-900">{formatBytes(customFilter.wasm_size_bytes)}</p>
						</div>
						<div>
							<p class="text-gray-500">Runtime</p>
							<p class="font-medium text-gray-900">{customFilter.runtime}</p>
						</div>
					</div>
					<p class="mt-3 text-xs text-gray-500">
						Binary cannot be changed. Upload a new custom filter to use a different WASM file.
					</p>
				</div>

				<!-- Metadata Section -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Metadata</h2>

					<div class="space-y-4">
						<!-- Name (Read-Only) -->
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Name</label>
							<div class="px-3 py-2 bg-gray-50 border border-gray-200 rounded-md text-gray-600">
								{customFilter.name}
							</div>
							<p class="mt-1 text-xs text-gray-500">Name cannot be changed</p>
						</div>

						<!-- Display Name -->
						<div>
							<label for="displayName" class="block text-sm font-medium text-gray-700 mb-1">
								Display Name <span class="text-red-500">*</span>
							</label>
							<input
								id="displayName"
								type="text"
								bind:value={displayName}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								placeholder="My Custom Filter"
							/>
						</div>

						<!-- Description -->
						<div>
							<label for="description" class="block text-sm font-medium text-gray-700 mb-1">
								Description
							</label>
							<textarea
								id="description"
								bind:value={description}
								rows="2"
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								placeholder="Describe what this filter does..."
							></textarea>
						</div>
					</div>
				</div>

				<!-- Configuration Schema Section -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Configuration Schema</h2>

					<div class="space-y-4">
						<!-- Config Schema -->
						<div>
							<label for="configSchema" class="block text-sm font-medium text-gray-700 mb-1">
								Config Schema (JSON Schema) <span class="text-red-500">*</span>
							</label>
							<textarea
								id="configSchema"
								bind:value={configSchemaText}
								oninput={handleConfigSchemaChange}
								rows="8"
								class="w-full px-3 py-2 border rounded-md font-mono text-sm focus:outline-none focus:ring-2 focus:ring-blue-500 {configSchemaError
									? 'border-red-300'
									: 'border-gray-300'}"
							></textarea>
							{#if configSchemaError}
								<p class="mt-1 text-xs text-red-600">{configSchemaError}</p>
							{/if}
						</div>

						<!-- Per-Route Config Schema -->
						<div>
							<label for="perRouteSchema" class="block text-sm font-medium text-gray-700 mb-1">
								Per-Route Config Schema (optional)
							</label>
							<textarea
								id="perRouteSchema"
								bind:value={perRouteSchemaText}
								oninput={handlePerRouteSchemaChange}
								rows="4"
								class="w-full px-3 py-2 border rounded-md font-mono text-sm focus:outline-none focus:ring-2 focus:ring-blue-500 {perRouteSchemaError
									? 'border-red-300'
									: 'border-gray-300'}"
								placeholder="Leave empty if no per-route config needed"
							></textarea>
							{#if perRouteSchemaError}
								<p class="mt-1 text-xs text-red-600">{perRouteSchemaError}</p>
							{/if}
						</div>

						<!-- UI Hints -->
						<div>
							<label for="uiHints" class="block text-sm font-medium text-gray-700 mb-1">
								UI Hints (optional)
							</label>
							<textarea
								id="uiHints"
								bind:value={uiHintsText}
								oninput={handleUiHintsChange}
								rows="3"
								class="w-full px-3 py-2 border rounded-md font-mono text-sm focus:outline-none focus:ring-2 focus:ring-blue-500 {uiHintsError
									? 'border-red-300'
									: 'border-gray-300'}"
							></textarea>
							{#if uiHintsError}
								<p class="mt-1 text-xs text-red-600">{uiHintsError}</p>
							{/if}
						</div>
					</div>
				</div>

				<!-- Advanced Options -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<button
						type="button"
						onclick={() => (showAdvanced = !showAdvanced)}
						class="flex items-center justify-between w-full text-left"
					>
						<h2 class="text-lg font-semibold text-gray-900">Advanced Options</h2>
						{#if showAdvanced}
							<ChevronUp class="h-5 w-5 text-gray-500" />
						{:else}
							<ChevronDown class="h-5 w-5 text-gray-500" />
						{/if}
					</button>

					{#if showAdvanced}
						<div class="mt-4 space-y-4">
							<!-- Attachment Points -->
							<div>
								<label class="block text-sm font-medium text-gray-700 mb-2">
									Attachment Points <span class="text-red-500">*</span>
								</label>
								<div class="flex gap-4">
									<label class="flex items-center gap-2">
										<input
											type="checkbox"
											checked={attachmentPoints.includes('listener')}
											onchange={() => toggleAttachmentPoint('listener')}
											class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
										/>
										<span class="text-sm text-gray-700">Listener</span>
									</label>
									<label class="flex items-center gap-2">
										<input
											type="checkbox"
											checked={attachmentPoints.includes('route')}
											onchange={() => toggleAttachmentPoint('route')}
											class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
										/>
										<span class="text-sm text-gray-700">Route</span>
									</label>
									<label class="flex items-center gap-2">
										<input
											type="checkbox"
											checked={attachmentPoints.includes('cluster')}
											onchange={() => toggleAttachmentPoint('cluster')}
											class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
										/>
										<span class="text-sm text-gray-700">Cluster</span>
									</label>
								</div>
							</div>

							<!-- Runtime and Failure Policy (Read-Only) -->
							<div class="grid grid-cols-2 gap-4">
								<div>
									<label class="block text-sm font-medium text-gray-700 mb-1">Runtime</label>
									<div
										class="px-3 py-2 bg-gray-50 border border-gray-200 rounded-md text-gray-600 text-sm"
									>
										{customFilter.runtime}
									</div>
								</div>
								<div>
									<label class="block text-sm font-medium text-gray-700 mb-1">Failure Policy</label>
									<div
										class="px-3 py-2 bg-gray-50 border border-gray-200 rounded-md text-gray-600 text-sm"
									>
										{customFilter.failure_policy}
									</div>
								</div>
							</div>
							<p class="text-xs text-gray-500">
								Runtime and failure policy cannot be changed after upload.
							</p>
						</div>
					{/if}
				</div>

				<!-- Submit Error -->
				{#if submitError}
					<div class="bg-red-50 border-l-4 border-red-500 rounded-md p-4">
						<div class="flex items-center gap-2">
							<AlertTriangle class="h-5 w-5 text-red-500" />
							<p class="text-red-800 text-sm">{submitError}</p>
						</div>
					</div>
				{/if}

				<!-- Actions -->
				<div class="flex gap-3">
					<Button variant="ghost" onclick={() => goto(`/custom-filters/${filterId}`)}>
						Cancel
					</Button>
					<Button
						variant="primary"
						onclick={handleSubmit}
						disabled={!isFormValid || isSubmitting || !hasChanges()}
					>
						{#if isSubmitting}
							<span class="flex items-center gap-2">
								<span
									class="animate-spin h-4 w-4 border-2 border-white border-t-transparent rounded-full"
								></span>
								Saving...
							</span>
						{:else}
							<Save class="h-4 w-4 mr-2" />
							Save Changes
						{/if}
					</Button>
				</div>
			</div>

			<!-- Right Column: Info -->
			<div class="space-y-6">
				<!-- Current Info -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Current Information</h2>
					<dl class="space-y-3 text-sm">
						<div>
							<dt class="text-gray-500">Filter Type</dt>
							<dd class="font-mono text-gray-900">{customFilter.filter_type}</dd>
						</div>
						<div>
							<dt class="text-gray-500">SHA256 Hash</dt>
							<dd class="font-mono text-xs text-gray-900 break-all">{customFilter.wasm_sha256}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Version</dt>
							<dd class="text-gray-900">{customFilter.version}</dd>
						</div>
						<div>
							<dt class="text-gray-500">Created By</dt>
							<dd class="text-gray-900">{customFilter.created_by || 'Unknown'}</dd>
						</div>
					</dl>
				</div>

				<!-- Change Detection -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Changes</h2>
					{#if hasChanges()}
						<div class="flex items-center gap-2 text-amber-600">
							<AlertTriangle class="h-5 w-5" />
							<span class="text-sm">You have unsaved changes</span>
						</div>
					{:else}
						<p class="text-sm text-gray-500">No changes made yet</p>
					{/if}
				</div>

				<!-- Help -->
				<div class="bg-blue-50 border border-blue-200 rounded-lg p-4">
					<h3 class="text-sm font-medium text-blue-800 mb-2">What can be edited?</h3>
					<ul class="text-sm text-blue-700 space-y-1 list-disc list-inside">
						<li>Display name and description</li>
						<li>Configuration schemas</li>
						<li>UI hints</li>
						<li>Attachment points</li>
					</ul>
					<p class="mt-3 text-sm text-blue-700">
						<strong>Cannot be changed:</strong> Name, WASM binary, runtime, failure policy.
						Upload a new filter to use different values.
					</p>
				</div>
			</div>
		</div>
	{/if}
</div>
