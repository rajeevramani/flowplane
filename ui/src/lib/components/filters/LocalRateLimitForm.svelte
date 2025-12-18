<script lang="ts">
	import type { LocalRateLimitConfig, TokenBucketConfig, RuntimeFractionalPercentConfig } from '$lib/api/types';
	import { Info, Settings, ChevronRight } from 'lucide-svelte';

	interface Props {
		config: LocalRateLimitConfig;
		onConfigChange: (config: LocalRateLimitConfig) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Initialize state from config
	let statPrefix = $state(config.stat_prefix || '');
	let maxTokens = $state(config.token_bucket?.max_tokens ?? 100);
	let tokensPerFill = $state(config.token_bucket?.tokens_per_fill ?? undefined);
	let fillIntervalMs = $state(config.token_bucket?.fill_interval_ms ?? 1000);
	let statusCode = $state(config.status_code ?? 429);
	// Default to false - global rate limit is the common use case
	// Per-connection rate limiting is only useful for specific scenarios
	let perDownstreamConnection = $state(config.per_downstream_connection ?? false);
	let rateLimitedAsResourceExhausted = $state(config.rate_limited_as_resource_exhausted ?? false);

	// New advanced fields
	let filterEnabledActive = $state(config.filter_enabled !== undefined);
	let filterEnabledNumerator = $state(config.filter_enabled?.numerator ?? 100);
	let filterEnforcedActive = $state(config.filter_enforced !== undefined);
	let filterEnforcedNumerator = $state(config.filter_enforced?.numerator ?? 100);
	let maxDynamicDescriptors = $state<number | undefined>(config.max_dynamic_descriptors);
	let alwaysConsumeDefaultTokenBucket = $state(config.always_consume_default_token_bucket ?? true);

	// Advanced options toggle
	let showAdvanced = $state(false);

	// Update parent when values change
	function updateParent() {
		const tokenBucket: TokenBucketConfig = {
			max_tokens: maxTokens,
			tokens_per_fill: tokensPerFill,
			fill_interval_ms: fillIntervalMs
		};

		// Build filter_enabled if active
		let filterEnabled: RuntimeFractionalPercentConfig | undefined = undefined;
		if (filterEnabledActive) {
			filterEnabled = {
				numerator: filterEnabledNumerator,
				denominator: 'hundred'
			};
		}

		// Build filter_enforced if active
		let filterEnforced: RuntimeFractionalPercentConfig | undefined = undefined;
		if (filterEnforcedActive) {
			filterEnforced = {
				numerator: filterEnforcedNumerator,
				denominator: 'hundred'
			};
		}

		onConfigChange({
			stat_prefix: statPrefix,
			token_bucket: tokenBucket,
			status_code: statusCode,
			filter_enabled: filterEnabled,
			filter_enforced: filterEnforced,
			per_downstream_connection: perDownstreamConnection,
			rate_limited_as_resource_exhausted: rateLimitedAsResourceExhausted,
			max_dynamic_descriptors: maxDynamicDescriptors,
			always_consume_default_token_bucket: alwaysConsumeDefaultTokenBucket
		});
	}

	// Calculate human-readable rate
	let rateDescription = $derived(() => {
		const tokensToAdd = tokensPerFill ?? maxTokens;
		const fillIntervalSec = fillIntervalMs / 1000;
		const ratePerSecond = tokensToAdd / fillIntervalSec;

		if (ratePerSecond >= 1) {
			return `~${ratePerSecond.toFixed(1)} requests/second`;
		} else {
			const ratePerMinute = ratePerSecond * 60;
			return `~${ratePerMinute.toFixed(1)} requests/minute`;
		}
	});
</script>

<div class="space-y-6">
	<!-- Info Box -->
	<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
		<div class="flex items-start gap-2">
			<Info class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0" />
			<div class="text-xs text-blue-700">
				<p class="font-medium">Local Rate Limiting</p>
				<p class="mt-1">
					Uses a token bucket algorithm to limit request rates. Each request consumes one token.
					Tokens are replenished at a fixed interval. When tokens are exhausted, requests are rate limited.
				</p>
			</div>
		</div>
	</div>

	<!-- Stat Prefix -->
	<div>
		<label class="block text-sm font-medium text-gray-700 mb-1">
			Stat Prefix <span class="text-red-500">*</span>
		</label>
		<input
			type="text"
			bind:value={statPrefix}
			oninput={updateParent}
			placeholder="e.g., api_rate_limit"
			class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		/>
		<p class="text-xs text-gray-500 mt-1">
			Prefix for statistics emitted by this filter
		</p>
	</div>

	<!-- Token Bucket Configuration -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Token Bucket Settings</h3>

		<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Max Tokens (Burst Size) <span class="text-red-500">*</span>
				</label>
				<input
					type="number"
					bind:value={maxTokens}
					oninput={updateParent}
					min="1"
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
				<p class="text-xs text-gray-500 mt-1">
					Maximum tokens in the bucket (allows bursts up to this size)
				</p>
			</div>

			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Tokens Per Fill
				</label>
				<input
					type="number"
					bind:value={tokensPerFill}
					oninput={updateParent}
					min="1"
					placeholder={String(maxTokens)}
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
				<p class="text-xs text-gray-500 mt-1">
					Tokens added per fill interval (defaults to max tokens)
				</p>
			</div>

			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Fill Interval (ms) <span class="text-red-500">*</span>
				</label>
				<input
					type="number"
					bind:value={fillIntervalMs}
					oninput={updateParent}
					min="1"
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
				<p class="text-xs text-gray-500 mt-1">
					Time between token refills in milliseconds
				</p>
			</div>

			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Status Code
				</label>
				<select
					bind:value={statusCode}
					onchange={updateParent}
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				>
					<option value={429}>429 Too Many Requests</option>
					<option value={503}>503 Service Unavailable</option>
					<option value={400}>400 Bad Request</option>
				</select>
				<p class="text-xs text-gray-500 mt-1">
					HTTP status code returned when rate limited
				</p>
			</div>
		</div>

		<!-- Rate Preview -->
		<div class="mt-4 p-3 bg-gray-50 rounded-md">
			<div class="flex items-center justify-between">
				<span class="text-sm text-gray-600">Effective Rate:</span>
				<span class="text-sm font-medium text-gray-900">{rateDescription()}</span>
			</div>
			<p class="text-xs text-gray-500 mt-1">
				With burst allowance of {maxTokens} requests
			</p>
		</div>
	</div>

	<!-- Advanced Options -->
	<div>
		<button
			type="button"
			onclick={() => (showAdvanced = !showAdvanced)}
			class="flex items-center gap-2 text-sm font-medium text-gray-600 hover:text-gray-900"
		>
			<Settings class="w-4 h-4" />
			<ChevronRight class="w-4 h-4 transition-transform {showAdvanced ? 'rotate-90' : ''}" />
			Advanced Options
		</button>

		{#if showAdvanced}
			<div class="mt-4 space-y-4 pl-6 border-l-2 border-gray-200">
				<!-- Filter Enabled (Percentage) -->
				<div class="space-y-2">
					<label class="flex items-center gap-3">
						<input
							type="checkbox"
							bind:checked={filterEnabledActive}
							onchange={updateParent}
							class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
						/>
						<div>
							<span class="text-sm font-medium text-gray-700">Filter Enabled (Runtime)</span>
							<p class="text-xs text-gray-500">
								Enable filter for a percentage of requests (for gradual rollout)
							</p>
						</div>
					</label>

					{#if filterEnabledActive}
						<div class="ml-7 flex items-center gap-2">
							<input
								type="number"
								min="0"
								max="100"
								bind:value={filterEnabledNumerator}
								oninput={updateParent}
								class="w-20 px-2 py-1 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							<span class="text-sm text-gray-600">% of requests</span>
						</div>
					{/if}
				</div>

				<!-- Filter Enforced (Percentage) -->
				<div class="space-y-2">
					<label class="flex items-center gap-3">
						<input
							type="checkbox"
							bind:checked={filterEnforcedActive}
							onchange={updateParent}
							class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
						/>
						<div>
							<span class="text-sm font-medium text-gray-700">Filter Enforced (Runtime)</span>
							<p class="text-xs text-gray-500">
								Enforce rate limiting for a percentage of rate-limited requests (shadow mode)
							</p>
						</div>
					</label>

					{#if filterEnforcedActive}
						<div class="ml-7 flex items-center gap-2">
							<input
								type="number"
								min="0"
								max="100"
								bind:value={filterEnforcedNumerator}
								oninput={updateParent}
								class="w-20 px-2 py-1 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							<span class="text-sm text-gray-600">% enforced</span>
						</div>
					{/if}
				</div>

				<!-- Max Dynamic Descriptors -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Max Dynamic Descriptors
					</label>
					<input
						type="number"
						min="0"
						bind:value={maxDynamicDescriptors}
						oninput={updateParent}
						placeholder="Unlimited"
						class="w-32 px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Maximum number of rate limit descriptors to track dynamically
					</p>
				</div>

				<!-- Per Downstream Connection -->
				<label class="flex items-center gap-3">
					<input
						type="checkbox"
						bind:checked={perDownstreamConnection}
						onchange={updateParent}
						class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
					/>
					<div>
						<span class="text-sm font-medium text-gray-700">Per Downstream Connection</span>
						<p class="text-xs text-gray-500">
							When enabled, each TCP connection gets its own token bucket. Leave disabled for global rate limiting across all connections (recommended for most use cases).
						</p>
					</div>
				</label>

				<!-- Resource Exhausted (gRPC) -->
				<label class="flex items-center gap-3">
					<input
						type="checkbox"
						bind:checked={rateLimitedAsResourceExhausted}
						onchange={updateParent}
						class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
					/>
					<div>
						<span class="text-sm font-medium text-gray-700">Resource Exhausted (gRPC)</span>
						<p class="text-xs text-gray-500">
							Return RESOURCE_EXHAUSTED instead of UNAVAILABLE for gRPC
						</p>
					</div>
				</label>

				<!-- Always Consume Default Token Bucket -->
				<label class="flex items-center gap-3">
					<input
						type="checkbox"
						bind:checked={alwaysConsumeDefaultTokenBucket}
						onchange={updateParent}
						class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
					/>
					<div>
						<span class="text-sm font-medium text-gray-700">Always Consume Default Token Bucket</span>
						<p class="text-xs text-gray-500">
							Consume tokens from the default bucket for every request. Required for rate limiting to work when no descriptors are configured.
						</p>
					</div>
				</label>
			</div>
		{/if}
	</div>
</div>
