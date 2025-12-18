<script lang="ts">
	import type { OutlierDetectionRequest } from '$lib/api/types';

	interface Props {
		config: OutlierDetectionRequest | null;
		onConfigChange: (config: OutlierDetectionRequest | null) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Presets for quick configuration
	type OutlierDetectionPreset = 'lenient' | 'standard' | 'strict' | 'custom';
	const presets: { value: OutlierDetectionPreset; label: string; config: OutlierDetectionRequest }[] = [
		{ value: 'lenient', label: 'Lenient', config: { consecutive5xx: 10, intervalSeconds: 30, baseEjectionTimeSeconds: 60, maxEjectionPercent: 5, minHosts: 1 } },
		{ value: 'standard', label: 'Standard', config: { consecutive5xx: 5, intervalSeconds: 10, baseEjectionTimeSeconds: 30, maxEjectionPercent: 10, minHosts: 1 } },
		{ value: 'strict', label: 'Strict', config: { consecutive5xx: 3, intervalSeconds: 5, baseEjectionTimeSeconds: 60, maxEjectionPercent: 25, minHosts: 1 } },
		{ value: 'custom', label: 'Custom', config: { consecutive5xx: 5, intervalSeconds: 10, baseEjectionTimeSeconds: 30, maxEjectionPercent: 10, minHosts: 1 } }
	];

	// State
	let enabled = $state(config !== null);
	let formConfig = $state<OutlierDetectionRequest>(
		config || { consecutive5xx: 5, intervalSeconds: 10, baseEjectionTimeSeconds: 30, maxEjectionPercent: 10, minHosts: 1 }
	);
	let selectedPreset = $state<OutlierDetectionPreset>('standard');
	let errors = $state<Record<string, string>>({});

	// Sync state from props
	$effect(() => {
		enabled = config !== null;
		if (config) {
			formConfig = { ...config };
			selectedPreset = detectPreset(config);
		}
	});

	function detectPreset(c: OutlierDetectionRequest): OutlierDetectionPreset {
		for (const preset of presets) {
			if (preset.value === 'custom') continue;
			const p = preset.config;
			if (
				c.consecutive5xx === p.consecutive5xx &&
				c.intervalSeconds === p.intervalSeconds &&
				c.baseEjectionTimeSeconds === p.baseEjectionTimeSeconds &&
				c.maxEjectionPercent === p.maxEjectionPercent &&
				c.minHosts === p.minHosts
			) {
				return preset.value;
			}
		}
		return 'custom';
	}

	function applyPreset(preset: OutlierDetectionPreset) {
		selectedPreset = preset;
		if (preset !== 'custom') {
			const presetConfig = presets.find(p => p.value === preset)?.config;
			if (presetConfig) {
				formConfig = { ...presetConfig };
				propagateChanges();
			}
		}
	}

	function validate(): Record<string, string> {
		const errs: Record<string, string> = {};

		if (formConfig.consecutive5xx !== undefined && (formConfig.consecutive5xx < 1 || formConfig.consecutive5xx > 1000)) {
			errs.consecutive5xx = 'Must be between 1 and 1,000';
		}
		if (formConfig.intervalSeconds !== undefined && (formConfig.intervalSeconds < 1 || formConfig.intervalSeconds > 300)) {
			errs.interval = 'Must be between 1 and 300 seconds';
		}
		if (formConfig.baseEjectionTimeSeconds !== undefined && (formConfig.baseEjectionTimeSeconds < 1 || formConfig.baseEjectionTimeSeconds > 3600)) {
			errs.ejectionTime = 'Must be between 1 and 3,600 seconds';
		}
		if (formConfig.maxEjectionPercent !== undefined && (formConfig.maxEjectionPercent < 1 || formConfig.maxEjectionPercent > 100)) {
			errs.maxPercent = 'Must be between 1 and 100';
		}
		if (formConfig.minHosts !== undefined && (formConfig.minHosts < 1 || formConfig.minHosts > 100)) {
			errs.minHosts = 'Must be between 1 and 100';
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
		errors = validate();
		if (Object.keys(errors).length > 0) {
			return;
		}

		onConfigChange({ ...formConfig });
	}

	function handleEnabledChange(e: Event) {
		enabled = (e.target as HTMLInputElement).checked;
		if (enabled) {
			// Initialize with defaults
			formConfig = { consecutive5xx: 5, intervalSeconds: 10, baseEjectionTimeSeconds: 30, maxEjectionPercent: 10, minHosts: 1 };
			selectedPreset = 'standard';
		}
		propagateChanges();
	}

	// Watch for form changes
	$effect(() => {
		if (enabled) {
			// Track changes
			void formConfig.consecutive5xx;
			void formConfig.intervalSeconds;
			void formConfig.baseEjectionTimeSeconds;
			void formConfig.maxEjectionPercent;
			void formConfig.minHosts;
			// Update preset detection
			selectedPreset = detectPreset(formConfig);
		}
	});
</script>

<div class="space-y-4">
	<!-- Enable Toggle -->
	<div class="flex items-center gap-3">
		<input
			type="checkbox"
			id="od-enabled"
			checked={enabled}
			onchange={handleEnabledChange}
			class="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
		/>
		<label for="od-enabled" class="text-sm font-medium text-gray-700">
			Enable Outlier Detection
		</label>
	</div>

	{#if !enabled}
		<p class="pl-7 text-sm text-gray-500">
			Passive health checking that automatically ejects unhealthy hosts based on observed request failures.
		</p>
	{:else}
		<div class="pl-7 space-y-4">
			<!-- Detection Settings -->
			<div class="space-y-3">
				<label class="block text-sm font-medium text-gray-700">Detection Settings</label>
				<div class="grid grid-cols-2 gap-4">
					<div>
						<label for="od-consecutive" class="block text-xs text-gray-600 mb-1">
							Consecutive 5xx Errors
						</label>
						<div class="flex items-center gap-2">
							<input
								id="od-consecutive"
								type="number"
								min="1"
								max="1000"
								bind:value={formConfig.consecutive5xx}
								oninput={propagateChanges}
								class="w-24 rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.consecutive5xx ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							<span class="text-sm text-gray-500">errors</span>
						</div>
						{#if errors.consecutive5xx}
							<p class="mt-1 text-xs text-red-600">{errors.consecutive5xx}</p>
						{:else}
							<p class="mt-1 text-xs text-gray-500">1-1,000</p>
						{/if}
					</div>
					<div>
						<label for="od-interval" class="block text-xs text-gray-600 mb-1">
							Detection Interval
						</label>
						<div class="flex items-center gap-2">
							<input
								id="od-interval"
								type="number"
								min="1"
								max="300"
								bind:value={formConfig.intervalSeconds}
								oninput={propagateChanges}
								class="w-24 rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.interval ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							<span class="text-sm text-gray-500">seconds</span>
						</div>
						{#if errors.interval}
							<p class="mt-1 text-xs text-red-600">{errors.interval}</p>
						{:else}
							<p class="mt-1 text-xs text-gray-500">1-300s</p>
						{/if}
					</div>
				</div>
			</div>

			<!-- Ejection Settings -->
			<div class="border-t border-gray-200 pt-4 space-y-3">
				<label class="block text-sm font-medium text-gray-700">Ejection Settings</label>
				<div class="grid grid-cols-2 gap-4">
					<div>
						<label for="od-ejection-time" class="block text-xs text-gray-600 mb-1">
							Base Ejection Time
						</label>
						<div class="flex items-center gap-2">
							<input
								id="od-ejection-time"
								type="number"
								min="1"
								max="3600"
								bind:value={formConfig.baseEjectionTimeSeconds}
								oninput={propagateChanges}
								class="w-24 rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.ejectionTime ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							<span class="text-sm text-gray-500">seconds</span>
						</div>
						{#if errors.ejectionTime}
							<p class="mt-1 text-xs text-red-600">{errors.ejectionTime}</p>
						{:else}
							<p class="mt-1 text-xs text-gray-500">1-3,600s</p>
						{/if}
					</div>
					<div>
						<label for="od-max-percent" class="block text-xs text-gray-600 mb-1">
							Max Ejection Percent
						</label>
						<div class="flex items-center gap-2">
							<input
								id="od-max-percent"
								type="number"
								min="1"
								max="100"
								bind:value={formConfig.maxEjectionPercent}
								oninput={propagateChanges}
								class="w-20 rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.maxPercent ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							<span class="text-sm text-gray-500">%</span>
						</div>
						{#if errors.maxPercent}
							<p class="mt-1 text-xs text-red-600">{errors.maxPercent}</p>
						{:else}
							<p class="mt-1 text-xs text-gray-500">1-100%</p>
						{/if}
					</div>
					<div class="col-span-2">
						<label for="od-min-hosts" class="block text-xs text-gray-600 mb-1">
							Minimum Healthy Hosts
						</label>
						<div class="flex items-center gap-2">
							<input
								id="od-min-hosts"
								type="number"
								min="1"
								max="100"
								bind:value={formConfig.minHosts}
								oninput={propagateChanges}
								class="w-20 rounded-md border px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500 {errors.minHosts ? 'border-red-300 bg-red-50' : 'border-gray-300'}"
							/>
							<span class="text-sm text-gray-500">hosts</span>
						</div>
						{#if errors.minHosts}
							<p class="mt-1 text-xs text-red-600">{errors.minHosts}</p>
						{:else}
							<p class="mt-1 text-xs text-gray-500">Minimum hosts to keep in rotation (1-100)</p>
						{/if}
					</div>
				</div>
			</div>

			<!-- Behavior Summary -->
			<div class="bg-gray-50 rounded-md p-3 border border-gray-200">
				<p class="text-sm text-gray-700">
					<span class="font-medium">Current behavior:</span>
					Hosts will be ejected after {formConfig.consecutive5xx || 5} consecutive 5xx errors.
					Initial ejection lasts {formConfig.baseEjectionTimeSeconds || 30}s (doubles with each ejection).
					At most {formConfig.maxEjectionPercent || 10}% of hosts can be ejected at once.
				</p>
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
		</div>
	{/if}
</div>
