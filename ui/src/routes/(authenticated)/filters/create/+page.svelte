<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { ArrowLeft } from 'lucide-svelte';
	import { selectedTeam } from '$lib/stores/team';
	import type {
		FilterType,
		FilterConfig,
		HeaderMutationConfig,
		HeaderMutationFilterConfig,
		JwtAuthenticationFilterConfig,
		LocalRateLimitConfig
	} from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import HeaderMutationConfigForm from '$lib/components/filters/HeaderMutationConfigForm.svelte';
	import JwtAuthConfigForm from '$lib/components/filters/JwtAuthConfigForm.svelte';
	import LocalRateLimitForm from '$lib/components/filters/LocalRateLimitForm.svelte';

	// Filter type metadata (using snake_case to match backend)
	const FILTER_TYPE_INFO: Record<
		FilterType,
		{ label: string; description: string; attachmentPoints: string[]; available: boolean }
	> = {
		header_mutation: {
			label: 'Header Mutation',
			description: 'Add, modify, or remove HTTP headers on requests and responses',
			attachmentPoints: ['Routes'],
			available: true
		},
		jwt_auth: {
			label: 'JWT Auth',
			description: 'JSON Web Token authentication',
			attachmentPoints: ['Routes', 'Listeners'],
			available: true
		},
		jwt_authn: {
			label: 'JWT Auth (Envoy)',
			description: 'JSON Web Token authentication (Envoy filter name)',
			attachmentPoints: ['Routes', 'Listeners'],
			available: false
		},
		cors: {
			label: 'CORS',
			description: 'Cross-Origin Resource Sharing configuration',
			attachmentPoints: ['Routes'],
			available: false
		},
		local_rate_limit: {
			label: 'Local Rate Limit',
			description: 'Local rate limiting with token bucket algorithm',
			attachmentPoints: ['Routes', 'Listeners'],
			available: true
		},
		rate_limit: {
			label: 'Rate Limit',
			description: 'Distributed rate limiting service',
			attachmentPoints: ['Routes', 'Listeners'],
			available: false
		},
		ext_authz: {
			label: 'External Auth',
			description: 'External authorization service',
			attachmentPoints: ['Routes', 'Listeners'],
			available: false
		}
	};

	// Form state
	let currentTeam = $state<string>('');
	let error = $state<string | null>(null);
	let isSubmitting = $state(false);

	let filterName = $state('');
	let filterDescription = $state('');
	let filterType = $state<FilterType>('header_mutation');

	// Header mutation config
	let headerMutationConfig = $state<HeaderMutationConfig>({
		requestHeadersToAdd: [],
		requestHeadersToRemove: [],
		responseHeadersToAdd: [],
		responseHeadersToRemove: []
	});

	// JWT auth config
	let jwtAuthConfig = $state<JwtAuthenticationFilterConfig>({
		providers: {},
		bypass_cors_preflight: false
	});

	// Local rate limit config
	let localRateLimitConfig = $state<LocalRateLimitConfig>({
		stat_prefix: '',
		token_bucket: {
			max_tokens: 100,
			tokens_per_fill: undefined,
			fill_interval_ms: 1000
		},
		status_code: 429
	});

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Get current filter type info
	let currentTypeInfo = $derived(FILTER_TYPE_INFO[filterType]);

	// Build the filter config based on filter type
	// Uses tagged enum format with snake_case to match backend Rust serialization
	function buildFilterConfig(): FilterConfig {
		if (filterType === 'jwt_auth') {
			return {
				type: 'jwt_auth',
				config: jwtAuthConfig
			};
		}

		if (filterType === 'local_rate_limit') {
			return {
				type: 'local_rate_limit',
				config: localRateLimitConfig
			};
		}

		// Default: header_mutation
		// Convert camelCase config to snake_case for backend
		const backendConfig: HeaderMutationFilterConfig = {
			request_headers_to_add: headerMutationConfig.requestHeadersToAdd,
			request_headers_to_remove: headerMutationConfig.requestHeadersToRemove,
			response_headers_to_add: headerMutationConfig.responseHeadersToAdd,
			response_headers_to_remove: headerMutationConfig.responseHeadersToRemove
		};

		return {
			type: 'header_mutation',
			config: backendConfig
		};
	}

	// Validate form
	function validateForm(): string | null {
		if (!filterName.trim()) {
			return 'Filter name is required';
		}

		if (filterName.length > 255) {
			return 'Filter name must be 255 characters or less';
		}

		// Type-specific validation
		if (filterType === 'jwt_auth') {
			return validateJwtAuthConfig();
		}

		if (filterType === 'local_rate_limit') {
			return validateLocalRateLimitConfig();
		}

		// Validate at least one header operation is configured
		const config = headerMutationConfig;
		const hasConfig =
			(config.requestHeadersToAdd && config.requestHeadersToAdd.length > 0) ||
			(config.requestHeadersToRemove && config.requestHeadersToRemove.length > 0) ||
			(config.responseHeadersToAdd && config.responseHeadersToAdd.length > 0) ||
			(config.responseHeadersToRemove && config.responseHeadersToRemove.length > 0);

		if (!hasConfig) {
			return 'Please configure at least one header operation';
		}

		// Validate header entries have both key and value
		const headersToAdd = [...(config.requestHeadersToAdd || []), ...(config.responseHeadersToAdd || [])];
		for (const header of headersToAdd) {
			if (!header.key.trim()) {
				return 'Header name cannot be empty';
			}
			if (!header.value.trim()) {
				return `Value for header "${header.key}" cannot be empty`;
			}
		}

		// Validate headers to remove have values
		const headersToRemove = [...(config.requestHeadersToRemove || []), ...(config.responseHeadersToRemove || [])];
		for (const header of headersToRemove) {
			if (!header.trim()) {
				return 'Header name to remove cannot be empty';
			}
		}

		return null;
	}

	// Validate local rate limit config
	function validateLocalRateLimitConfig(): string | null {
		if (!localRateLimitConfig.stat_prefix.trim()) {
			return 'Stat prefix is required for rate limit filter';
		}

		if (!localRateLimitConfig.token_bucket) {
			return 'Token bucket configuration is required';
		}

		if (localRateLimitConfig.token_bucket.max_tokens < 1) {
			return 'Max tokens must be at least 1';
		}

		if (localRateLimitConfig.token_bucket.fill_interval_ms < 1) {
			return 'Fill interval must be at least 1ms';
		}

		return null;
	}

	// Validate JWT auth config
	function validateJwtAuthConfig(): string | null {
		if (Object.keys(jwtAuthConfig.providers).length === 0) {
			return 'At least one JWT provider is required';
		}

		for (const [name, provider] of Object.entries(jwtAuthConfig.providers)) {
			if (!name.trim()) {
				return 'Provider name cannot be empty';
			}

			// Validate JWKS source
			if (provider.jwks.type === 'remote') {
				if (!provider.jwks.http_uri.uri.trim()) {
					return `Provider "${name}": JWKS URI is required`;
				}
				if (!provider.jwks.http_uri.cluster.trim()) {
					return `Provider "${name}": Cluster name is required for remote JWKS`;
				}
			} else if (provider.jwks.type === 'local') {
				if (!provider.jwks.inline_string?.trim() && !provider.jwks.filename?.trim()) {
					return `Provider "${name}": Either inline JWKS or filename is required`;
				}
			}
		}

		return null;
	}

	// Handle form submission
	async function handleSubmit() {
		error = null;
		const validationError = validateForm();
		if (validationError) {
			error = validationError;
			return;
		}

		isSubmitting = true;

		try {
			await apiClient.createFilter({
				name: filterName.trim(),
				filterType: filterType,
				description: filterDescription.trim() || undefined,
				config: buildFilterConfig(),
				team: currentTeam
			});
			goto('/filters');
		} catch (e) {
			console.error('Create filter failed:', e);
			error = e instanceof Error ? e.message : 'Failed to create filter';
		} finally {
			isSubmitting = false;
		}
	}

	// Handle cancel
	function handleCancel() {
		goto('/filters');
	}

	// Handle config change
	function handleConfigChange(config: HeaderMutationConfig) {
		headerMutationConfig = config;
	}

	// Handle JWT config change
	function handleJwtConfigChange(config: JwtAuthenticationFilterConfig) {
		jwtAuthConfig = config;
	}

	// Handle local rate limit config change
	function handleLocalRateLimitConfigChange(config: LocalRateLimitConfig) {
		localRateLimitConfig = config;
	}
</script>

<div class="max-w-4xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
	<!-- Page Header with Back Button -->
	<div class="mb-6">
		<div class="flex items-center gap-4 mb-2">
			<button
				onclick={handleCancel}
				class="p-2 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded-md transition-colors"
			>
				<ArrowLeft class="w-5 h-5" />
			</button>
			<div>
				<h1 class="text-2xl font-bold text-gray-900">Create Filter</h1>
				<p class="mt-1 text-sm text-gray-600">Create a reusable filter configuration</p>
			</div>
		</div>
	</div>

	<!-- Error Message -->
	{#if error}
		<div class="mb-6 bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{/if}

	<!-- Basic Information -->
	<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
		<h2 class="text-lg font-semibold text-gray-900 mb-4">Basic Information</h2>
		<div class="space-y-4">
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Filter Name <span class="text-red-500">*</span>
				</label>
				<input
					type="text"
					bind:value={filterName}
					placeholder="e.g., add-security-headers"
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
				<p class="text-xs text-gray-500 mt-1">
					A unique name to identify this filter within your team
				</p>
			</div>

			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Description</label>
				<textarea
					bind:value={filterDescription}
					placeholder="Optional description of what this filter does"
					rows="2"
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				></textarea>
			</div>

			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Team</label>
				<input
					type="text"
					value={currentTeam}
					disabled
					class="w-full px-3 py-2 border border-gray-200 rounded-md bg-gray-50 text-gray-500"
				/>
				<p class="text-xs text-gray-500 mt-1">Filters are scoped to your current team</p>
			</div>
		</div>
	</div>

	<!-- Filter Type -->
	<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
		<h2 class="text-lg font-semibold text-gray-900 mb-4">Filter Type</h2>
		<div class="space-y-4">
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Select Filter Type <span class="text-red-500">*</span>
				</label>
				<select
					bind:value={filterType}
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				>
					{#each Object.entries(FILTER_TYPE_INFO) as [type, info]}
						<option value={type} disabled={!info.available}>
							{info.label} - {info.description}
							{#if !info.available}(Coming soon){/if}
						</option>
					{/each}
				</select>
			</div>

			<!-- Attachment Points Info -->
			<div class="p-3 bg-gray-50 border border-gray-200 rounded-md">
				<div class="flex items-center justify-between">
					<span class="text-sm font-medium text-gray-700">Can attach to:</span>
					<div class="flex gap-1">
						{#each currentTypeInfo.attachmentPoints as point}
							<Badge variant="blue">{point}</Badge>
						{/each}
					</div>
				</div>
				<p class="text-sm text-gray-500 mt-2">{currentTypeInfo.description}</p>
			</div>
		</div>
	</div>

	<!-- Configuration -->
	<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
		{#if filterType === 'header_mutation'}
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Header Mutation Configuration</h2>
			<HeaderMutationConfigForm config={headerMutationConfig} onConfigChange={handleConfigChange} />
		{:else if filterType === 'jwt_auth'}
			<h2 class="text-lg font-semibold text-gray-900 mb-4">JWT Authentication Configuration</h2>
			<JwtAuthConfigForm config={jwtAuthConfig} onConfigChange={handleJwtConfigChange} />
		{:else if filterType === 'local_rate_limit'}
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Local Rate Limit Configuration</h2>
			<LocalRateLimitForm config={localRateLimitConfig} onConfigChange={handleLocalRateLimitConfigChange} />
		{:else}
			<div class="text-center py-8 text-gray-500">
				<p>Configuration for this filter type is not yet available.</p>
			</div>
		{/if}
	</div>

	<!-- Action Buttons -->
	<div class="flex justify-end gap-3">
		<Button onclick={handleCancel} variant="secondary" disabled={isSubmitting}>
			Cancel
		</Button>
		<Button onclick={handleSubmit} variant="primary" disabled={isSubmitting}>
			{isSubmitting ? 'Creating...' : 'Create Filter'}
		</Button>
	</div>
</div>
