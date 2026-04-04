<script lang="ts">
	import type { RateLimitConfig } from '$lib/api/types';
	import { RateLimitConfigSchema } from '$lib/schemas/filter-configs';
	import { Info, Settings, ChevronRight } from 'lucide-svelte';

	interface Props {
		config: RateLimitConfig;
		onConfigChange: (config: RateLimitConfig) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Initialize state from config
	let domain = $state(config.domain ?? '');
	let grpcType = $state<'envoy' | 'google'>(
		config.rate_limit_service?.grpc_service?.google_grpc ? 'google' : 'envoy'
	);
	let envoyClusterName = $state(
		config.rate_limit_service?.grpc_service?.envoy_grpc?.cluster_name ?? ''
	);
	let envoyAuthority = $state(
		config.rate_limit_service?.grpc_service?.envoy_grpc?.authority ?? ''
	);
	let googleTargetUri = $state(
		config.rate_limit_service?.grpc_service?.google_grpc?.target_uri ?? ''
	);
	let googleStatPrefix = $state(
		config.rate_limit_service?.grpc_service?.google_grpc?.stat_prefix ?? ''
	);
	let serviceTimeout = $state(config.rate_limit_service?.grpc_service?.timeout ?? '');

	// Optional fields
	let stage = $state<number | undefined>(config.stage);
	let requestType = $state<string>(config.request_type ?? 'both');
	let timeout = $state(config.timeout ?? '');
	let failureModeDeny = $state(config.failure_mode_deny ?? false);
	let rateLimitedAsResourceExhausted = $state(
		config.rate_limited_as_resource_exhausted ?? false
	);
	let enableXRatelimitHeaders = $state<string>(
		config.enable_x_ratelimit_headers ?? 'OFF'
	);

	// Advanced toggle
	let showAdvanced = $state(false);

	// Validation errors
	let validationErrors = $state<string[]>([]);

	function updateParent() {
		const grpcService: RateLimitConfig['rate_limit_service']['grpc_service'] = {};

		if (grpcType === 'envoy') {
			grpcService.envoy_grpc = {
				cluster_name: envoyClusterName
			};
			if (envoyAuthority.trim()) grpcService.envoy_grpc.authority = envoyAuthority.trim();
		} else {
			grpcService.google_grpc = {
				target_uri: googleTargetUri
			};
			if (googleStatPrefix.trim())
				grpcService.google_grpc.stat_prefix = googleStatPrefix.trim();
		}
		if (serviceTimeout.trim()) grpcService.timeout = serviceTimeout.trim();

		const newConfig: RateLimitConfig = {
			domain,
			rate_limit_service: {
				grpc_service: grpcService
			}
		};

		if (stage !== undefined && stage >= 0) newConfig.stage = stage;
		if (requestType !== 'both') newConfig.request_type = requestType as RateLimitConfig['request_type'];
		if (timeout.trim()) newConfig.timeout = timeout.trim();
		if (failureModeDeny) newConfig.failure_mode_deny = true;
		if (rateLimitedAsResourceExhausted) newConfig.rate_limited_as_resource_exhausted = true;
		if (enableXRatelimitHeaders !== 'OFF')
			newConfig.enable_x_ratelimit_headers = enableXRatelimitHeaders as RateLimitConfig['enable_x_ratelimit_headers'];

		const result = RateLimitConfigSchema.safeParse(newConfig);
		validationErrors = result.success
			? []
			: result.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`);

		onConfigChange(newConfig);
	}
</script>

<div class="space-y-6">
	<!-- Info Box -->
	<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
		<div class="flex items-start gap-2">
			<Info class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0" />
			<div class="text-xs text-blue-700">
				<p class="font-medium">External Rate Limiting</p>
				<p class="mt-1">
					Distributed rate limiting using an external gRPC rate limit service. For simpler
					per-instance rate limiting, use the Local Rate Limit filter instead.
				</p>
			</div>
		</div>
	</div>

	<!-- Validation Errors -->
	{#if validationErrors.length > 0}
		<div class="rounded-lg border border-red-200 bg-red-50 p-3">
			<ul class="text-xs text-red-700 list-disc list-inside space-y-0.5">
				{#each validationErrors as err}
					<li>{err}</li>
				{/each}
			</ul>
		</div>
	{/if}

	<!-- Domain -->
	<div>
		<label class="block text-sm font-medium text-gray-700 mb-1">
			Domain <span class="text-red-500">*</span>
		</label>
		<input
			type="text"
			bind:value={domain}
			oninput={updateParent}
			placeholder="e.g., my-service-ratelimit"
			class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		/>
		<p class="text-xs text-gray-500 mt-1">
			Rate limit domain identifier. Must match the domain in your rate limit service configuration.
		</p>
	</div>

	<!-- gRPC Service Configuration -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">
			Rate Limit Service <span class="text-red-500">*</span>
		</h3>

		<!-- gRPC Type Selector -->
		<div class="mb-4">
			<label class="block text-sm font-medium text-gray-700 mb-1">Service Type</label>
			<div class="flex gap-4">
				<label class="flex items-center gap-2">
					<input
						type="radio"
						bind:group={grpcType}
						value="envoy"
						onchange={updateParent}
						class="h-4 w-4 text-blue-600 border-gray-300 focus:ring-blue-500"
					/>
					<span class="text-sm text-gray-700">Envoy gRPC (cluster-based)</span>
				</label>
				<label class="flex items-center gap-2">
					<input
						type="radio"
						bind:group={grpcType}
						value="google"
						onchange={updateParent}
						class="h-4 w-4 text-blue-600 border-gray-300 focus:ring-blue-500"
					/>
					<span class="text-sm text-gray-700">Google gRPC (direct)</span>
				</label>
			</div>
		</div>

		{#if grpcType === 'envoy'}
			<div class="space-y-4">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Cluster Name <span class="text-red-500">*</span>
					</label>
					<input
						type="text"
						bind:value={envoyClusterName}
						oninput={updateParent}
						placeholder="e.g., rate_limit_cluster"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Name of the Envoy cluster configured for the rate limit service
					</p>
				</div>
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Authority</label>
					<input
						type="text"
						bind:value={envoyAuthority}
						oninput={updateParent}
						placeholder="Optional HTTP/2 authority"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
				</div>
			</div>
		{:else}
			<div class="space-y-4">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Target URI <span class="text-red-500">*</span>
					</label>
					<input
						type="text"
						bind:value={googleTargetUri}
						oninput={updateParent}
						placeholder="e.g., dns:///ratelimit.service:8081"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						gRPC target URI for the rate limit service
					</p>
				</div>
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Stat Prefix</label>
					<input
						type="text"
						bind:value={googleStatPrefix}
						oninput={updateParent}
						placeholder="Optional stats prefix"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
				</div>
			</div>
		{/if}

		<!-- Service Timeout -->
		<div class="mt-4">
			<label class="block text-sm font-medium text-gray-700 mb-1">Service Timeout</label>
			<input
				type="text"
				bind:value={serviceTimeout}
				oninput={updateParent}
				placeholder="e.g., 0.25s"
				class="w-40 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
			/>
			<p class="text-xs text-gray-500 mt-1">
				Timeout for rate limit service calls (e.g., 0.25s, 500ms)
			</p>
		</div>
	</div>

	<!-- Request Type -->
	<div>
		<label class="block text-sm font-medium text-gray-700 mb-1">Request Type</label>
		<select
			bind:value={requestType}
			onchange={updateParent}
			class="w-48 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		>
			<option value="both">Both (internal & external)</option>
			<option value="internal">Internal only</option>
			<option value="external">External only</option>
		</select>
		<p class="text-xs text-gray-500 mt-1">Which request types are subject to rate limiting</p>
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
				<!-- Stage -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Stage</label>
					<input
						type="number"
						bind:value={stage}
						oninput={updateParent}
						min="0"
						max="10"
						placeholder="0"
						class="w-24 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Rate limit stage number (0-10). Multiple stages allow multi-pass rate limiting.
					</p>
				</div>

				<!-- Timeout -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Request Timeout</label>
					<input
						type="text"
						bind:value={timeout}
						oninput={updateParent}
						placeholder="e.g., 0.25s"
						class="w-40 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Per-request timeout for rate limit checks
					</p>
				</div>

				<!-- Failure Mode Deny -->
				<label class="flex items-center gap-3">
					<input
						type="checkbox"
						bind:checked={failureModeDeny}
						onchange={updateParent}
						class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
					/>
					<div>
						<span class="text-sm font-medium text-gray-700">Failure Mode Deny</span>
						<p class="text-xs text-gray-500">
							Deny requests when the rate limit service is unavailable (default: allow)
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

				<!-- X-RateLimit Headers -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						X-RateLimit Response Headers
					</label>
					<select
						bind:value={enableXRatelimitHeaders}
						onchange={updateParent}
						class="w-56 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					>
						<option value="OFF">Disabled</option>
						<option value="DRAFT_VERSION_03">Draft v3 (recommended)</option>
					</select>
					<p class="text-xs text-gray-500 mt-1">
						Include X-RateLimit-Limit and X-RateLimit-Remaining headers in responses
					</p>
				</div>
			</div>
		{/if}
	</div>
</div>
