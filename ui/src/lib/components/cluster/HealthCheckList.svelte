<script lang="ts">
	import type { HealthCheckRequest } from '$lib/api/types';

	interface Props {
		checks: HealthCheckRequest[];
		onChecksChange: (checks: HealthCheckRequest[]) => void;
	}

	let { checks, onChecksChange }: Props = $props();

	// Health check types
	const healthCheckTypes = [
		{ value: 'http', label: 'HTTP' },
		{ value: 'tcp', label: 'TCP' }
	];

	// HTTP methods for health checks
	const httpMethods = ['GET', 'HEAD', 'POST'];

	// Presets for quick configuration
	type HealthCheckPreset = 'standard' | 'aggressive' | 'conservative' | 'custom';
	const presets: { value: HealthCheckPreset; label: string; config: Partial<HealthCheckRequest> }[] = [
		{ value: 'standard', label: 'Standard', config: { intervalSeconds: 10, timeoutSeconds: 5, healthyThreshold: 2, unhealthyThreshold: 3 } },
		{ value: 'aggressive', label: 'Aggressive', config: { intervalSeconds: 5, timeoutSeconds: 2, healthyThreshold: 1, unhealthyThreshold: 2 } },
		{ value: 'conservative', label: 'Conservative', config: { intervalSeconds: 30, timeoutSeconds: 10, healthyThreshold: 3, unhealthyThreshold: 5 } },
		{ value: 'custom', label: 'Custom', config: {} }
	];

	// Edit state
	let editingIndex = $state<number | null>(null);
	let editForm = $state<HealthCheckRequest>({
		type: 'http',
		path: '/health',
		intervalSeconds: 10,
		timeoutSeconds: 5,
		healthyThreshold: 2,
		unhealthyThreshold: 3,
		expectedStatuses: [200]
	});
	let selectedPreset = $state<HealthCheckPreset>('standard');

	// Validation errors
	let errors = $state<Record<string, string>>({});

	// Derived: check if timeout < interval
	let timeoutValid = $derived(
		(editForm.timeoutSeconds || 5) < (editForm.intervalSeconds || 10)
	);

	// Derived: check if path is valid for HTTP
	let pathValid = $derived(
		editForm.type !== 'http' || (editForm.path && editForm.path.startsWith('/'))
	);

	function getDefaultHealthCheck(): HealthCheckRequest {
		return {
			type: 'http',
			path: '/health',
			intervalSeconds: 10,
			timeoutSeconds: 5,
			healthyThreshold: 2,
			unhealthyThreshold: 3,
			expectedStatuses: [200]
		};
	}

	function startAdd() {
		editingIndex = -1; // -1 means adding new
		editForm = getDefaultHealthCheck();
		selectedPreset = 'standard';
		errors = {};
	}

	function startEdit(index: number) {
		editingIndex = index;
		editForm = { ...checks[index] };
		selectedPreset = detectPreset(editForm);
		errors = {};
	}

	function cancelEdit() {
		editingIndex = null;
		errors = {};
	}

	function detectPreset(config: HealthCheckRequest): HealthCheckPreset {
		for (const preset of presets) {
			if (preset.value === 'custom') continue;
			const p = preset.config;
			if (
				config.intervalSeconds === p.intervalSeconds &&
				config.timeoutSeconds === p.timeoutSeconds &&
				config.healthyThreshold === p.healthyThreshold &&
				config.unhealthyThreshold === p.unhealthyThreshold
			) {
				return preset.value;
			}
		}
		return 'custom';
	}

	function applyPreset(preset: HealthCheckPreset) {
		selectedPreset = preset;
		if (preset !== 'custom') {
			const presetConfig = presets.find(p => p.value === preset)?.config;
			if (presetConfig) {
				editForm = { ...editForm, ...presetConfig };
			}
		}
	}

	function validate(): boolean {
		const newErrors: Record<string, string> = {};

		// Type validation
		if (!editForm.type) {
			newErrors.type = 'Health check type is required';
		}

		// Path validation for HTTP
		if (editForm.type === 'http') {
			if (!editForm.path) {
				newErrors.path = 'Path is required for HTTP health checks';
			} else if (!editForm.path.startsWith('/')) {
				newErrors.path = 'Path must start with /';
			} else if (editForm.path.includes('..')) {
				newErrors.path = 'Path cannot contain ..';
			} else if (editForm.path.length > 200) {
				newErrors.path = 'Path must be 200 characters or less';
			}
		}

		// Interval validation
		if (!editForm.intervalSeconds || editForm.intervalSeconds < 1 || editForm.intervalSeconds > 300) {
			newErrors.interval = 'Interval must be between 1 and 300 seconds';
		}

		// Timeout validation
		if (!editForm.timeoutSeconds || editForm.timeoutSeconds < 1 || editForm.timeoutSeconds > 60) {
			newErrors.timeout = 'Timeout must be between 1 and 60 seconds';
		} else if (editForm.timeoutSeconds >= (editForm.intervalSeconds || 10)) {
			newErrors.timeout = 'Timeout must be less than interval';
		}

		// Threshold validation
		if (!editForm.healthyThreshold || editForm.healthyThreshold < 1 || editForm.healthyThreshold > 10) {
			newErrors.healthyThreshold = 'Healthy threshold must be between 1 and 10';
		}
		if (!editForm.unhealthyThreshold || editForm.unhealthyThreshold < 1 || editForm.unhealthyThreshold > 10) {
			newErrors.unhealthyThreshold = 'Unhealthy threshold must be between 1 and 10';
		}

		errors = newErrors;
		return Object.keys(newErrors).length === 0;
	}

	function saveHealthCheck() {
		if (!validate()) return;

		const newCheck: HealthCheckRequest = {
			type: editForm.type,
			intervalSeconds: editForm.intervalSeconds,
			timeoutSeconds: editForm.timeoutSeconds,
			healthyThreshold: editForm.healthyThreshold,
			unhealthyThreshold: editForm.unhealthyThreshold
		};

		// Add HTTP-specific fields
		if (editForm.type === 'http') {
			newCheck.path = editForm.path;
			if (editForm.host) newCheck.host = editForm.host;
			if (editForm.method && editForm.method !== 'GET') newCheck.method = editForm.method;
			if (editForm.expectedStatuses && editForm.expectedStatuses.length > 0) {
				newCheck.expectedStatuses = editForm.expectedStatuses;
			}
		}

		let newChecks: HealthCheckRequest[];
		if (editingIndex === -1) {
			// Adding new
			newChecks = [...checks, newCheck];
		} else if (editingIndex !== null) {
			// Editing existing
			newChecks = checks.map((c, i) => i === editingIndex ? newCheck : c);
		} else {
			return;
		}

		onChecksChange(newChecks);
		editingIndex = null;
	}

	function removeHealthCheck(index: number) {
		onChecksChange(checks.filter((_, i) => i !== index));
	}

	function addExpectedStatus() {
		editForm.expectedStatuses = [...(editForm.expectedStatuses || []), 200];
	}

	function removeExpectedStatus(index: number) {
		if (editForm.expectedStatuses && editForm.expectedStatuses.length > 1) {
			editForm.expectedStatuses = editForm.expectedStatuses.filter((_, i) => i !== index);
		}
	}

	function updateExpectedStatus(index: number, value: number) {
		if (editForm.expectedStatuses) {
			editForm.expectedStatuses = editForm.expectedStatuses.map((s, i) => i === index ? value : s);
		}
	}

	function formatHealthCheck(check: HealthCheckRequest): string {
		if (check.type === 'http') {
			return `HTTP ${check.path || '/'}`;
		}
		return 'TCP';
	}
</script>

<div class="space-y-4">
	{#if checks.length === 0 && editingIndex === null}
		<!-- Empty state -->
		<div class="text-center py-6 bg-gray-50 rounded-lg border border-dashed border-gray-300">
			<svg class="mx-auto h-10 w-10 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M4.5 12.75l6 6 9-13.5" />
			</svg>
			<p class="mt-2 text-sm text-gray-600">No health checks configured</p>
			<p class="text-xs text-gray-500">Add active health checking to monitor endpoint health.</p>
			<button
				type="button"
				onclick={startAdd}
				class="mt-3 inline-flex items-center gap-1 px-3 py-1.5 text-sm font-medium text-blue-600 bg-blue-50 rounded-md hover:bg-blue-100"
			>
				<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
				</svg>
				Add Health Check
			</button>
		</div>
	{:else}
		<!-- List of health checks -->
		{#each checks as check, index}
			<div class="flex items-center justify-between p-3 bg-gray-50 rounded-lg border border-gray-200">
				<div class="flex items-center gap-3">
					<span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium {check.type === 'http' ? 'bg-green-100 text-green-800' : 'bg-blue-100 text-blue-800'}">
						{check.type?.toUpperCase()}
					</span>
					<span class="text-sm font-medium text-gray-700">
						{formatHealthCheck(check)}
					</span>
					<span class="text-xs text-gray-500">
						{check.intervalSeconds}s interval | {check.timeoutSeconds}s timeout
					</span>
				</div>
				<div class="flex items-center gap-2">
					<button
						type="button"
						onclick={() => startEdit(index)}
						class="p-1 text-gray-400 hover:text-blue-600 rounded"
						title="Edit"
					>
						<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
						</svg>
					</button>
					<button
						type="button"
						onclick={() => removeHealthCheck(index)}
						class="p-1 text-gray-400 hover:text-red-600 rounded"
						title="Delete"
					>
						<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
						</svg>
					</button>
				</div>
			</div>
		{/each}

		{#if editingIndex === null}
			<button
				type="button"
				onclick={startAdd}
				class="flex items-center gap-1 text-sm text-blue-600 hover:text-blue-700"
			>
				<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
				</svg>
				Add Health Check
			</button>
		{/if}
	{/if}

	<!-- Add/Edit Form -->
	{#if editingIndex !== null}
		<div class="border border-blue-200 rounded-lg p-4 bg-blue-50/30 space-y-4">
			<div class="flex items-center justify-between">
				<h4 class="text-sm font-medium text-gray-900">
					{editingIndex === -1 ? 'Add Health Check' : 'Edit Health Check'}
				</h4>
				<button
					type="button"
					onclick={cancelEdit}
					class="p-1 text-gray-400 hover:text-gray-600 rounded"
				>
					<svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
						<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
					</svg>
				</button>
			</div>

			<!-- Health Check Type -->
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-2">Type</label>
				<div class="flex gap-2">
					{#each healthCheckTypes as hcType}
						<button
							type="button"
							onclick={() => editForm.type = hcType.value}
							class="px-4 py-2 text-sm font-medium rounded-md transition-colors {editForm.type === hcType.value
								? 'bg-blue-600 text-white'
								: 'bg-white border border-gray-300 text-gray-700 hover:bg-gray-50'}"
						>
							{hcType.label}
						</button>
					{/each}
				</div>
				{#if errors.type}
					<p class="mt-1 text-xs text-red-600">{errors.type}</p>
				{/if}
			</div>

			<!-- HTTP-specific fields -->
			{#if editForm.type === 'http'}
				<div class="grid grid-cols-2 gap-4">
					<div class="col-span-2">
						<label for="hc-path" class="block text-sm font-medium text-gray-700 mb-1">
							Path <span class="text-red-500">*</span>
						</label>
						<input
							id="hc-path"
							type="text"
							bind:value={editForm.path}
							placeholder="/health"
							class="w-full rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.path ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
						/>
						{#if errors.path}
							<p class="mt-1 text-xs text-red-600">{errors.path}</p>
						{/if}
					</div>
					<div>
						<label for="hc-host" class="block text-sm font-medium text-gray-700 mb-1">
							Host <span class="text-xs text-gray-400">(optional)</span>
						</label>
						<input
							id="hc-host"
							type="text"
							bind:value={editForm.host}
							placeholder="Override Host header"
							class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
						/>
					</div>
					<div>
						<label for="hc-method" class="block text-sm font-medium text-gray-700 mb-1">Method</label>
						<select
							id="hc-method"
							bind:value={editForm.method}
							class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
						>
							{#each httpMethods as m}
								<option value={m}>{m}</option>
							{/each}
						</select>
					</div>
				</div>

				<!-- Expected Status Codes -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Expected Status Codes
					</label>
					<div class="flex flex-wrap gap-2">
						{#each editForm.expectedStatuses || [200] as status, index}
							<div class="flex items-center gap-1">
								<input
									type="number"
									min="100"
									max="599"
									value={status}
									oninput={(e) => updateExpectedStatus(index, Number((e.target as HTMLInputElement).value))}
									class="w-20 rounded-md border border-gray-300 px-2 py-1 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
								/>
								{#if (editForm.expectedStatuses?.length || 0) > 1}
									<button
										type="button"
										onclick={() => removeExpectedStatus(index)}
										class="p-1 text-gray-400 hover:text-red-600 rounded"
									>
										<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
											<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
										</svg>
									</button>
								{/if}
							</div>
						{/each}
						<button
							type="button"
							onclick={addExpectedStatus}
							class="flex items-center gap-1 px-2 py-1 text-xs text-blue-600 hover:text-blue-700"
						>
							<svg class="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
							</svg>
							Add
						</button>
					</div>
				</div>
			{/if}

			<!-- Timing Settings -->
			<div class="border-t border-gray-200 pt-4">
				<label class="block text-sm font-medium text-gray-700 mb-2">Timing</label>
				<div class="grid grid-cols-2 gap-4">
					<div>
						<label for="hc-interval" class="block text-xs text-gray-600 mb-1">
							Interval <span class="text-gray-400">(1-300s)</span>
						</label>
						<div class="flex items-center gap-2">
							<input
								id="hc-interval"
								type="number"
								min="1"
								max="300"
								bind:value={editForm.intervalSeconds}
								class="w-20 rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.interval ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							<span class="text-sm text-gray-500">seconds</span>
						</div>
						{#if errors.interval}
							<p class="mt-1 text-xs text-red-600">{errors.interval}</p>
						{/if}
					</div>
					<div>
						<label for="hc-timeout" class="block text-xs text-gray-600 mb-1">
							Timeout <span class="text-gray-400">(1-60s)</span>
						</label>
						<div class="flex items-center gap-2">
							<input
								id="hc-timeout"
								type="number"
								min="1"
								max="60"
								bind:value={editForm.timeoutSeconds}
								class="w-20 rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.timeout ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							<span class="text-sm text-gray-500">seconds</span>
						</div>
						{#if errors.timeout}
							<p class="mt-1 text-xs text-red-600">{errors.timeout}</p>
						{/if}
					</div>
				</div>
			</div>

			<!-- Thresholds -->
			<div class="border-t border-gray-200 pt-4">
				<label class="block text-sm font-medium text-gray-700 mb-2">Thresholds</label>
				<div class="grid grid-cols-2 gap-4">
					<div>
						<label for="hc-healthy" class="block text-xs text-gray-600 mb-1">
							Healthy after <span class="text-gray-400">(1-10)</span>
						</label>
						<div class="flex items-center gap-2">
							<input
								id="hc-healthy"
								type="number"
								min="1"
								max="10"
								bind:value={editForm.healthyThreshold}
								class="w-16 rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.healthyThreshold ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							<span class="text-sm text-gray-500">successes</span>
						</div>
						{#if errors.healthyThreshold}
							<p class="mt-1 text-xs text-red-600">{errors.healthyThreshold}</p>
						{/if}
					</div>
					<div>
						<label for="hc-unhealthy" class="block text-xs text-gray-600 mb-1">
							Unhealthy after <span class="text-gray-400">(1-10)</span>
						</label>
						<div class="flex items-center gap-2">
							<input
								id="hc-unhealthy"
								type="number"
								min="1"
								max="10"
								bind:value={editForm.unhealthyThreshold}
								class="w-16 rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.unhealthyThreshold ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							<span class="text-sm text-gray-500">failures</span>
						</div>
						{#if errors.unhealthyThreshold}
							<p class="mt-1 text-xs text-red-600">{errors.unhealthyThreshold}</p>
						{/if}
					</div>
				</div>
			</div>

			<!-- Presets -->
			<div class="border-t border-gray-200 pt-4">
				<label class="block text-xs text-gray-600 mb-2">Quick Settings</label>
				<div class="flex flex-wrap gap-2">
					{#each presets as preset}
						<button
							type="button"
							onclick={() => applyPreset(preset.value)}
							class="px-3 py-1.5 text-sm rounded-full border transition-colors {selectedPreset === preset.value
								? 'bg-blue-100 border-blue-500 text-blue-700'
								: 'border-gray-300 text-gray-600 hover:bg-gray-50'}"
						>
							{preset.label}
						</button>
					{/each}
				</div>
			</div>

			<!-- Actions -->
			<div class="flex justify-end gap-2 pt-2">
				<button
					type="button"
					onclick={cancelEdit}
					class="px-3 py-1.5 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
				>
					Cancel
				</button>
				<button
					type="button"
					onclick={saveHealthCheck}
					class="px-3 py-1.5 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
				>
					{editingIndex === -1 ? 'Add' : 'Save'}
				</button>
			</div>
		</div>
	{/if}
</div>
