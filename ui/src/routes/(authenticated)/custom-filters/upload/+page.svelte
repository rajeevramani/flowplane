<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { ArrowLeft, Upload, CheckCircle, XCircle, AlertTriangle, ChevronDown, ChevronUp } from 'lucide-svelte';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';

	let currentTeam = $state<string>('');

	// File upload state
	let fileInput = $state<HTMLInputElement | null>(null);
	let isDragging = $state(false);
	let wasmFile = $state<File | null>(null);
	let wasmBinaryBase64 = $state<string>('');
	let wasmSize = $state<number>(0);
	let wasmValid = $state<boolean | null>(null);
	let wasmError = $state<string | null>(null);

	// Form fields
	let name = $state('');
	let displayName = $state('');
	let description = $state('');
	let configSchemaText = $state('{\n  "type": "object",\n  "properties": {},\n  "additionalProperties": true\n}');
	let perRouteSchemaText = $state('');
	let uiHintsText = $state('');

	// Advanced options
	let showAdvanced = $state(false);
	let attachmentPoints = $state<string[]>(['listener', 'route']);
	let runtime = $state('envoy.wasm.runtime.v8');
	let failurePolicy = $state('FAIL_CLOSED');

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
		currentTeam = value;
	});

	// Validate WASM magic bytes
	function validateWasmMagic(arrayBuffer: ArrayBuffer): boolean {
		if (arrayBuffer.byteLength < 4) return false;
		const magic = new Uint8Array(arrayBuffer.slice(0, 4));
		// WASM magic: 0x00 0x61 0x73 0x6D ("\0asm")
		return magic[0] === 0x00 && magic[1] === 0x61 && magic[2] === 0x73 && magic[3] === 0x6d;
	}

	// Convert ArrayBuffer to Base64
	function arrayBufferToBase64(buffer: ArrayBuffer): string {
		const bytes = new Uint8Array(buffer);
		let binary = '';
		for (let i = 0; i < bytes.byteLength; i++) {
			binary += String.fromCharCode(bytes[i]);
		}
		return btoa(binary);
	}

	// Handle file selection
	function handleFileSelect(event: Event) {
		const target = event.target as HTMLInputElement;
		if (target.files && target.files[0]) {
			processFile(target.files[0]);
		}
	}

	// Handle drag over
	function handleDragOver(event: DragEvent) {
		event.preventDefault();
		isDragging = true;
	}

	// Handle drag leave
	function handleDragLeave() {
		isDragging = false;
	}

	// Handle drop
	function handleDrop(event: DragEvent) {
		event.preventDefault();
		isDragging = false;

		if (event.dataTransfer?.files && event.dataTransfer.files[0]) {
			processFile(event.dataTransfer.files[0]);
		}
	}

	// Process uploaded file
	async function processFile(file: File) {
		wasmFile = file;
		wasmError = null;
		wasmValid = null;

		// Check file extension
		if (!file.name.endsWith('.wasm')) {
			wasmError = 'File must have .wasm extension';
			wasmValid = false;
			return;
		}

		try {
			const arrayBuffer = await file.arrayBuffer();
			wasmSize = arrayBuffer.byteLength;

			// Validate magic bytes
			if (!validateWasmMagic(arrayBuffer)) {
				wasmError = 'Invalid WASM binary: magic bytes not found';
				wasmValid = false;
				return;
			}

			// Convert to base64
			wasmBinaryBase64 = arrayBufferToBase64(arrayBuffer);
			wasmValid = true;

			// Auto-populate name from filename if empty
			if (!name) {
				const baseName = file.name.replace('.wasm', '');
				name = baseName.replace(/[^a-zA-Z0-9_-]/g, '_');
			}
			if (!displayName) {
				displayName = file.name.replace('.wasm', '');
			}
		} catch (e) {
			wasmError = e instanceof Error ? e.message : 'Failed to read file';
			wasmValid = false;
		}
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

	// Format bytes
	function formatBytes(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	// Check if form is valid
	let isFormValid = $derived(
		wasmValid === true &&
		name.trim().length > 0 &&
		displayName.trim().length > 0 &&
		configSchemaValid === true &&
		(perRouteSchemaValid === null || perRouteSchemaValid === true) &&
		(uiHintsValid === null || uiHintsValid === true) &&
		attachmentPoints.length > 0
	);

	// Handle form submission
	async function handleSubmit() {
		if (!isFormValid || !currentTeam) return;

		isSubmitting = true;
		submitError = null;

		try {
			const request = {
				name: name.trim(),
				display_name: displayName.trim(),
				description: description.trim() || undefined,
				wasm_binary_base64: wasmBinaryBase64,
				config_schema: JSON.parse(configSchemaText),
				per_route_config_schema: perRouteSchemaText.trim() ? JSON.parse(perRouteSchemaText) : undefined,
				ui_hints: uiHintsText.trim() ? JSON.parse(uiHintsText) : undefined,
				attachment_points: attachmentPoints,
				runtime,
				failure_policy: failurePolicy
			};

			await apiClient.createCustomWasmFilter(currentTeam, request);
			goto('/custom-filters');
		} catch (e) {
			submitError = e instanceof Error ? e.message : 'Failed to upload custom filter';
		} finally {
			isSubmitting = false;
		}
	}

	// Clear file
	function clearFile() {
		wasmFile = null;
		wasmBinaryBase64 = '';
		wasmSize = 0;
		wasmValid = null;
		wasmError = null;
		if (fileInput) {
			fileInput.value = '';
		}
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="flex items-center gap-4 mb-6">
		<a href="/custom-filters" class="text-blue-600 hover:text-blue-800" aria-label="Back to custom filters">
			<ArrowLeft class="h-6 w-6" />
		</a>
		<h1 class="text-2xl font-bold text-gray-900">Upload Custom Filter</h1>
	</div>

	<div class="grid grid-cols-1 lg:grid-cols-2 gap-8">
		<!-- Left Column: Upload & Form -->
		<div class="space-y-6">
			<!-- File Upload Section -->
			<div class="bg-white rounded-lg shadow-md p-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4">WASM Binary</h2>

				{#if wasmFile && wasmValid}
					<!-- File Selected -->
					<div class="border-2 border-green-300 bg-green-50 rounded-lg p-4">
						<div class="flex items-center justify-between">
							<div class="flex items-center gap-3">
								<CheckCircle class="h-8 w-8 text-green-600" />
								<div>
									<p class="font-medium text-gray-900">{wasmFile.name}</p>
									<p class="text-sm text-gray-600">{formatBytes(wasmSize)}</p>
								</div>
							</div>
							<button
								onclick={clearFile}
								class="text-gray-500 hover:text-gray-700"
								title="Remove file"
							>
								<XCircle class="h-5 w-5" />
							</button>
						</div>
					</div>
				{:else}
					<!-- Drag & Drop Zone -->
					<div
						role="button"
						tabindex="0"
						class="border-2 border-dashed rounded-lg p-8 text-center transition-colors {isDragging
							? 'border-blue-500 bg-blue-50'
							: wasmError
								? 'border-red-300 bg-red-50'
								: 'border-gray-300 hover:border-gray-400'}"
						ondragover={handleDragOver}
						ondragleave={handleDragLeave}
						ondrop={handleDrop}
						onkeydown={(e) => e.key === 'Enter' && fileInput?.click()}
					>
						<Upload class="mx-auto h-12 w-12 text-gray-400" />
						<p class="mt-2 text-sm text-gray-600">
							Drag and drop your .wasm file here, or
						</p>
						<button
							type="button"
							onclick={() => fileInput?.click()}
							class="mt-2 px-4 py-2 text-sm font-medium text-blue-600 hover:text-blue-800"
						>
							Browse Files
						</button>
						<input
							type="file"
							bind:this={fileInput}
							onchange={handleFileSelect}
							accept=".wasm"
							class="hidden"
						/>
					</div>

					{#if wasmError}
						<div class="mt-2 flex items-center gap-2 text-red-600">
							<XCircle class="h-4 w-4" />
							<span class="text-sm">{wasmError}</span>
						</div>
					{/if}
				{/if}
			</div>

			<!-- Metadata Section -->
			<div class="bg-white rounded-lg shadow-md p-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Metadata</h2>

				<div class="space-y-4">
					<!-- Name -->
					<div>
						<label for="name" class="block text-sm font-medium text-gray-700 mb-1">
							Name <span class="text-red-500">*</span>
						</label>
						<input
							id="name"
							type="text"
							bind:value={name}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							placeholder="my-custom-filter"
						/>
						<p class="mt-1 text-xs text-gray-500">
							Alphanumeric, underscores, and hyphens only
						</p>
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
							rows="6"
							class="w-full px-3 py-2 border rounded-md font-mono text-sm focus:outline-none focus:ring-2 focus:ring-blue-500 {configSchemaError
								? 'border-red-300'
								: 'border-gray-300'}"
							placeholder={'{"type": "object", "properties": {}}'}
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
							placeholder={'{"formLayout": "flat"}'}
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

						<!-- Runtime -->
						<div>
							<label for="runtime" class="block text-sm font-medium text-gray-700 mb-1">
								WASM Runtime
							</label>
							<select
								id="runtime"
								bind:value={runtime}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							>
								<option value="envoy.wasm.runtime.v8">V8 (Recommended)</option>
								<option value="envoy.wasm.runtime.wamr">WAMR</option>
								<option value="envoy.wasm.runtime.wasmtime">Wasmtime</option>
								<option value="envoy.wasm.runtime.null">Null (Testing)</option>
							</select>
						</div>

						<!-- Failure Policy -->
						<div>
							<label for="failurePolicy" class="block text-sm font-medium text-gray-700 mb-1">
								Failure Policy
							</label>
							<select
								id="failurePolicy"
								bind:value={failurePolicy}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							>
								<option value="FAIL_CLOSED">Fail Closed (Block on error)</option>
								<option value="FAIL_OPEN">Fail Open (Allow on error)</option>
							</select>
							<p class="mt-1 text-xs text-gray-500">
								Determines behavior when the WASM module fails to load or execute
							</p>
						</div>
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
				<Button variant="ghost" onclick={() => goto('/custom-filters')}>
					Cancel
				</Button>
				<Button
					variant="primary"
					onclick={handleSubmit}
					disabled={!isFormValid || isSubmitting}
				>
					{#if isSubmitting}
						<span class="flex items-center gap-2">
							<span class="animate-spin h-4 w-4 border-2 border-white border-t-transparent rounded-full"></span>
							Uploading...
						</span>
					{:else}
						<Upload class="h-4 w-4 mr-2" />
						Upload Filter
					{/if}
				</Button>
			</div>
		</div>

		<!-- Right Column: Preview -->
		<div class="space-y-6">
			<!-- Binary Info -->
			{#if wasmFile}
				<div class="bg-white rounded-lg shadow-md p-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Binary Info</h2>
					<div class="space-y-3">
						<div class="flex items-center gap-2">
							{#if wasmValid}
								<CheckCircle class="h-5 w-5 text-green-600" />
								<span class="text-sm text-green-700">Valid WASM binary</span>
							{:else}
								<XCircle class="h-5 w-5 text-red-600" />
								<span class="text-sm text-red-700">Invalid WASM binary</span>
							{/if}
						</div>
						<div class="grid grid-cols-2 gap-4 text-sm">
							<div>
								<p class="text-gray-500">File Name</p>
								<p class="font-medium text-gray-900">{wasmFile.name}</p>
							</div>
							<div>
								<p class="text-gray-500">Size</p>
								<p class="font-medium text-gray-900">{formatBytes(wasmSize)}</p>
							</div>
						</div>
					</div>
				</div>
			{/if}

			<!-- Schema Preview -->
			{#if configSchemaValid}
				<div class="bg-white rounded-lg shadow-md p-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Schema Preview</h2>
					<pre class="bg-gray-900 text-gray-100 rounded-md p-4 overflow-auto max-h-64 text-xs"><code>{JSON.stringify(JSON.parse(configSchemaText), null, 2)}</code></pre>
				</div>
			{/if}

			<!-- Validation Status -->
			<div class="bg-white rounded-lg shadow-md p-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Validation Status</h2>
				<div class="space-y-2">
					<div class="flex items-center gap-2">
						{#if wasmValid}
							<CheckCircle class="h-4 w-4 text-green-600" />
							<span class="text-sm text-green-700">WASM binary valid</span>
						{:else if wasmValid === false}
							<XCircle class="h-4 w-4 text-red-600" />
							<span class="text-sm text-red-700">WASM binary invalid</span>
						{:else}
							<div class="h-4 w-4 rounded-full bg-gray-200"></div>
							<span class="text-sm text-gray-500">No file uploaded</span>
						{/if}
					</div>
					<div class="flex items-center gap-2">
						{#if name.trim()}
							<CheckCircle class="h-4 w-4 text-green-600" />
							<span class="text-sm text-green-700">Name provided</span>
						{:else}
							<div class="h-4 w-4 rounded-full bg-gray-200"></div>
							<span class="text-sm text-gray-500">Name required</span>
						{/if}
					</div>
					<div class="flex items-center gap-2">
						{#if displayName.trim()}
							<CheckCircle class="h-4 w-4 text-green-600" />
							<span class="text-sm text-green-700">Display name provided</span>
						{:else}
							<div class="h-4 w-4 rounded-full bg-gray-200"></div>
							<span class="text-sm text-gray-500">Display name required</span>
						{/if}
					</div>
					<div class="flex items-center gap-2">
						{#if configSchemaValid}
							<CheckCircle class="h-4 w-4 text-green-600" />
							<span class="text-sm text-green-700">Config schema valid</span>
						{:else if configSchemaError}
							<XCircle class="h-4 w-4 text-red-600" />
							<span class="text-sm text-red-700">Config schema invalid</span>
						{:else}
							<div class="h-4 w-4 rounded-full bg-gray-200"></div>
							<span class="text-sm text-gray-500">Config schema required</span>
						{/if}
					</div>
					<div class="flex items-center gap-2">
						{#if attachmentPoints.length > 0}
							<CheckCircle class="h-4 w-4 text-green-600" />
							<span class="text-sm text-green-700">Attachment points configured</span>
						{:else}
							<XCircle class="h-4 w-4 text-red-600" />
							<span class="text-sm text-red-700">At least one attachment point required</span>
						{/if}
					</div>
				</div>
			</div>

			<!-- Tips -->
			<div class="bg-blue-50 border border-blue-200 rounded-lg p-4">
				<h3 class="text-sm font-medium text-blue-800 mb-2">Tips</h3>
				<ul class="text-sm text-blue-700 space-y-1 list-disc list-inside">
					<li>The config schema defines what configuration your filter accepts</li>
					<li>Use per-route schema if your filter supports different settings per route</li>
					<li>V8 runtime is recommended for most use cases</li>
					<li>FAIL_CLOSED is safer for production environments</li>
				</ul>
			</div>
		</div>
	</div>
</div>
