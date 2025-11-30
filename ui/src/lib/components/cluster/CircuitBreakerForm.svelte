<script lang="ts">
	import type { CircuitBreakersRequest, CircuitBreakerThresholdsRequest } from '$lib/api/types';

	interface Props {
		config: CircuitBreakersRequest | null;
		onConfigChange: (config: CircuitBreakersRequest | null) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Presets for quick configuration
	type CircuitBreakerPreset = 'low' | 'standard' | 'high' | 'custom';
	const presets: { value: CircuitBreakerPreset; label: string; config: CircuitBreakerThresholdsRequest }[] = [
		{ value: 'low', label: 'Low Traffic', config: { maxConnections: 256, maxPendingRequests: 128, maxRequests: 256, maxRetries: 1 } },
		{ value: 'standard', label: 'Standard', config: { maxConnections: 1024, maxPendingRequests: 1024, maxRequests: 1024, maxRetries: 3 } },
		{ value: 'high', label: 'High Traffic', config: { maxConnections: 4096, maxPendingRequests: 2048, maxRequests: 4096, maxRetries: 5 } },
		{ value: 'custom', label: 'Custom', config: { maxConnections: 1024, maxPendingRequests: 1024, maxRequests: 1024, maxRetries: 3 } }
	];

	// State
	let enabled = $state(config !== null && config.default !== undefined);
	let defaultConfig = $state<CircuitBreakerThresholdsRequest>(
		config?.default || { maxConnections: 1024, maxPendingRequests: 1024, maxRequests: 1024, maxRetries: 3 }
	);
	let highEnabled = $state(config?.high !== undefined);
	let highConfig = $state<CircuitBreakerThresholdsRequest>(
		config?.high || { maxConnections: 1024, maxPendingRequests: 1024, maxRequests: 1024, maxRetries: 3 }
	);
	let selectedPreset = $state<CircuitBreakerPreset>('standard');
	let priorityTab = $state<'default' | 'high'>('default');
	let errors = $state<Record<string, string>>({});

	// Sync state from props
	$effect(() => {
		enabled = config !== null && config.default !== undefined;
		if (config?.default) {
			defaultConfig = { ...config.default };
			selectedPreset = detectPreset(config.default);
		}
		highEnabled = config?.high !== undefined;
		if (config?.high) {
			highConfig = { ...config.high };
		}
	});

	function detectPreset(c: CircuitBreakerThresholdsRequest): CircuitBreakerPreset {
		for (const preset of presets) {
			if (preset.value === 'custom') continue;
			const p = preset.config;
			if (
				c.maxConnections === p.maxConnections &&
				c.maxPendingRequests === p.maxPendingRequests &&
				c.maxRequests === p.maxRequests &&
				c.maxRetries === p.maxRetries
			) {
				return preset.value;
			}
		}
		return 'custom';
	}

	function applyPreset(preset: CircuitBreakerPreset) {
		selectedPreset = preset;
		if (preset !== 'custom') {
			const presetConfig = presets.find(p => p.value === preset)?.config;
			if (presetConfig) {
				defaultConfig = { ...presetConfig };
				propagateChanges();
			}
		}
	}

	function resetToDefaults() {
		defaultConfig = { maxConnections: 1024, maxPendingRequests: 1024, maxRequests: 1024, maxRetries: 3 };
		selectedPreset = 'standard';
		propagateChanges();
	}

	function validate(cfg: CircuitBreakerThresholdsRequest, prefix: string): Record<string, string> {
		const errs: Record<string, string> = {};

		if (cfg.maxConnections !== undefined && (cfg.maxConnections < 1 || cfg.maxConnections > 10000)) {
			errs[`${prefix}Connections`] = 'Must be between 1 and 10,000';
		}
		if (cfg.maxPendingRequests !== undefined && (cfg.maxPendingRequests < 1 || cfg.maxPendingRequests > 10000)) {
			errs[`${prefix}Pending`] = 'Must be between 1 and 10,000';
		}
		if (cfg.maxRequests !== undefined && (cfg.maxRequests < 1 || cfg.maxRequests > 10000)) {
			errs[`${prefix}Requests`] = 'Must be between 1 and 10,000';
		}
		if (cfg.maxRetries !== undefined && (cfg.maxRetries < 0 || cfg.maxRetries > 10)) {
			errs[`${prefix}Retries`] = 'Must be between 0 and 10';
		}

		return errs;
	}

	function propagateChanges() {
		if (!enabled) {
			onConfigChange(null);
			errors = {};
			return;
		}

		// Validate
		const defaultErrors = validate(defaultConfig, 'default');
		const highErrors = highEnabled ? validate(highConfig, 'high') : {};
		errors = { ...defaultErrors, ...highErrors };

		if (Object.keys(errors).length > 0) {
			return;
		}

		const newConfig: CircuitBreakersRequest = {
			default: { ...defaultConfig }
		};

		if (highEnabled) {
			newConfig.high = { ...highConfig };
		}

		onConfigChange(newConfig);
	}

	function handleEnabledChange(e: Event) {
		enabled = (e.target as HTMLInputElement).checked;
		if (enabled) {
			// Initialize with defaults
			defaultConfig = { maxConnections: 1024, maxPendingRequests: 1024, maxRequests: 1024, maxRetries: 3 };
			selectedPreset = 'standard';
		}
		propagateChanges();
	}

	function handleHighEnabledChange(e: Event) {
		highEnabled = (e.target as HTMLInputElement).checked;
		if (highEnabled) {
			// Initialize high with same as default
			highConfig = { ...defaultConfig };
		}
		propagateChanges();
	}

	// Watch for form changes
	$effect(() => {
		if (enabled) {
			// Track changes to defaultConfig and highConfig
			void defaultConfig.maxConnections;
			void defaultConfig.maxPendingRequests;
			void defaultConfig.maxRequests;
			void defaultConfig.maxRetries;
			if (highEnabled) {
				void highConfig.maxConnections;
				void highConfig.maxPendingRequests;
				void highConfig.maxRequests;
				void highConfig.maxRetries;
			}
			// Update preset detection
			selectedPreset = detectPreset(defaultConfig);
		}
	});
</script>

<div class="space-y-4">
	<!-- Enable Toggle -->
	<div class="flex items-center gap-3">
		<input
			type="checkbox"
			id="cb-enabled"
			checked={enabled}
			onchange={handleEnabledChange}
			class="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
		/>
		<label for="cb-enabled" class="text-sm font-medium text-gray-700">
			Enable Circuit Breaker
		</label>
	</div>

	{#if !enabled}
		<p class="pl-7 text-sm text-gray-500">
			Circuit breakers help prevent cascade failures by limiting concurrent connections and requests.
		</p>
	{:else}
		<div class="pl-7 space-y-4">
			<!-- Priority Tabs -->
			<div class="flex gap-4 border-b border-gray-200">
				<button
					type="button"
					onclick={() => priorityTab = 'default'}
					class="pb-2 text-sm font-medium transition-colors {priorityTab === 'default'
						? 'text-blue-600 border-b-2 border-blue-600 -mb-px'
						: 'text-gray-500 hover:text-gray-700'}"
				>
					Default Priority
				</button>
				<button
					type="button"
					onclick={() => priorityTab = 'high'}
					class="pb-2 text-sm font-medium transition-colors {priorityTab === 'high'
						? 'text-blue-600 border-b-2 border-blue-600 -mb-px'
						: 'text-gray-500 hover:text-gray-700'}"
				>
					High Priority
					{#if highEnabled}
						<span class="ml-1 inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium bg-yellow-100 text-yellow-700">
							On
						</span>
					{/if}
				</button>
			</div>

			{#if priorityTab === 'default'}
				<!-- Default Priority Configuration -->
				<div class="space-y-4">
					<div class="grid grid-cols-2 gap-4">
						<div>
							<label for="cb-max-connections" class="block text-sm font-medium text-gray-700 mb-1">
								Max Connections
							</label>
							<input
								id="cb-max-connections"
								type="number"
								min="1"
								max="10000"
								bind:value={defaultConfig.maxConnections}
								oninput={propagateChanges}
								class="w-full rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.defaultConnections ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							{#if errors.defaultConnections}
								<p class="mt-1 text-xs text-red-600">{errors.defaultConnections}</p>
							{:else}
								<p class="mt-1 text-xs text-gray-500">1-10,000</p>
							{/if}
						</div>
						<div>
							<label for="cb-max-pending" class="block text-sm font-medium text-gray-700 mb-1">
								Max Pending Requests
							</label>
							<input
								id="cb-max-pending"
								type="number"
								min="1"
								max="10000"
								bind:value={defaultConfig.maxPendingRequests}
								oninput={propagateChanges}
								class="w-full rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.defaultPending ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							{#if errors.defaultPending}
								<p class="mt-1 text-xs text-red-600">{errors.defaultPending}</p>
							{:else}
								<p class="mt-1 text-xs text-gray-500">1-10,000</p>
							{/if}
						</div>
						<div>
							<label for="cb-max-requests" class="block text-sm font-medium text-gray-700 mb-1">
								Max Requests
							</label>
							<input
								id="cb-max-requests"
								type="number"
								min="1"
								max="10000"
								bind:value={defaultConfig.maxRequests}
								oninput={propagateChanges}
								class="w-full rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.defaultRequests ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							{#if errors.defaultRequests}
								<p class="mt-1 text-xs text-red-600">{errors.defaultRequests}</p>
							{:else}
								<p class="mt-1 text-xs text-gray-500">1-10,000</p>
							{/if}
						</div>
						<div>
							<label for="cb-max-retries" class="block text-sm font-medium text-gray-700 mb-1">
								Max Retries
							</label>
							<input
								id="cb-max-retries"
								type="number"
								min="0"
								max="10"
								bind:value={defaultConfig.maxRetries}
								oninput={propagateChanges}
								class="w-full rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.defaultRetries ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							{#if errors.defaultRetries}
								<p class="mt-1 text-xs text-red-600">{errors.defaultRetries}</p>
							{:else}
								<p class="mt-1 text-xs text-gray-500">0-10</p>
							{/if}
						</div>
					</div>

					<button
						type="button"
						onclick={resetToDefaults}
						class="text-sm text-gray-500 hover:text-gray-700 underline"
					>
						Reset to Defaults
					</button>

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
				</div>

			{:else}
				<!-- High Priority Configuration -->
				<div class="space-y-4">
					<div class="flex items-center gap-3">
						<input
							type="checkbox"
							id="cb-high-enabled"
							checked={highEnabled}
							onchange={handleHighEnabledChange}
							class="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
						/>
						<label for="cb-high-enabled" class="text-sm font-medium text-gray-700">
							Configure separate high-priority thresholds
						</label>
					</div>

					{#if !highEnabled}
						<p class="text-sm text-gray-500">
							High priority circuit breakers are used for requests marked as high priority (e.g., health checks, admin operations).
						</p>
					{:else}
						<div class="grid grid-cols-2 gap-4">
							<div>
								<label for="cb-high-connections" class="block text-sm font-medium text-gray-700 mb-1">
									Max Connections
								</label>
								<input
									id="cb-high-connections"
									type="number"
									min="1"
									max="10000"
									bind:value={highConfig.maxConnections}
									oninput={propagateChanges}
									class="w-full rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.highConnections ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
								/>
								{#if errors.highConnections}
									<p class="mt-1 text-xs text-red-600">{errors.highConnections}</p>
								{:else}
									<p class="mt-1 text-xs text-gray-500">1-10,000</p>
								{/if}
							</div>
							<div>
								<label for="cb-high-pending" class="block text-sm font-medium text-gray-700 mb-1">
									Max Pending Requests
								</label>
								<input
									id="cb-high-pending"
									type="number"
									min="1"
									max="10000"
									bind:value={highConfig.maxPendingRequests}
									oninput={propagateChanges}
									class="w-full rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.highPending ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
								/>
								{#if errors.highPending}
									<p class="mt-1 text-xs text-red-600">{errors.highPending}</p>
								{:else}
									<p class="mt-1 text-xs text-gray-500">1-10,000</p>
								{/if}
							</div>
							<div>
								<label for="cb-high-requests" class="block text-sm font-medium text-gray-700 mb-1">
									Max Requests
								</label>
								<input
									id="cb-high-requests"
									type="number"
									min="1"
									max="10000"
									bind:value={highConfig.maxRequests}
									oninput={propagateChanges}
									class="w-full rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.highRequests ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
								/>
								{#if errors.highRequests}
									<p class="mt-1 text-xs text-red-600">{errors.highRequests}</p>
								{:else}
									<p class="mt-1 text-xs text-gray-500">1-10,000</p>
								{/if}
							</div>
							<div>
								<label for="cb-high-retries" class="block text-sm font-medium text-gray-700 mb-1">
									Max Retries
								</label>
								<input
									id="cb-high-retries"
									type="number"
									min="0"
									max="10"
									bind:value={highConfig.maxRetries}
									oninput={propagateChanges}
									class="w-full rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.highRetries ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
								/>
								{#if errors.highRetries}
									<p class="mt-1 text-xs text-red-600">{errors.highRetries}</p>
								{:else}
									<p class="mt-1 text-xs text-gray-500">0-10</p>
								{/if}
							</div>
						</div>
					{/if}
				</div>
			{/if}
		</div>
	{/if}
</div>
