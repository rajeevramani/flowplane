<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { ArrowLeft } from 'lucide-svelte';
	import type { FilterResponse, FilterConfig, HeaderMutationConfig, HeaderMutationFilterConfig, JwtAuthenticationFilterConfig, LocalRateLimitConfig } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import HeaderMutationConfigForm from '$lib/components/filters/HeaderMutationConfigForm.svelte';
	import JwtAuthConfigForm from '$lib/components/filters/JwtAuthConfigForm.svelte';
	import LocalRateLimitForm from '$lib/components/filters/LocalRateLimitForm.svelte';

	// Page state
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let isSubmitting = $state(false);
	let filter = $state<FilterResponse | null>(null);

	// Form state
	let filterName = $state('');
	let filterDescription = $state('');
	let headerMutationConfig = $state<HeaderMutationConfig>({
		requestHeadersToAdd: [],
		requestHeadersToRemove: [],
		responseHeadersToAdd: [],
		responseHeadersToRemove: []
	});
	let jwtAuthConfig = $state<JwtAuthenticationFilterConfig>({
		providers: {},
		bypass_cors_preflight: false
	});
	let localRateLimitConfig = $state<LocalRateLimitConfig>({
		stat_prefix: '',
		token_bucket: {
			max_tokens: 100,
			tokens_per_fill: undefined,
			fill_interval_ms: 1000
		},
		status_code: 429
	});

	// Get filter ID from route params (always defined for this route)
	function getFilterId(): string {
		return $page.params.id ?? '';
	}

	onMount(async () => {
		await loadFilter();
	});

	async function loadFilter() {
		isLoading = true;
		error = null;

		try {
			const data = await apiClient.getFilter(getFilterId());
			filter = data;

			// Populate form fields
			filterName = data.name;
			filterDescription = data.description || '';

			// Load config from tagged enum format based on filter type
			// Backend returns: { type: 'header_mutation' | 'jwt_auth' | 'local_rate_limit', config: { ... } }
			if (data.config.type === 'header_mutation') {
				const backendConfig = data.config.config;
				headerMutationConfig = {
					requestHeadersToAdd: backendConfig.request_headers_to_add || [],
					requestHeadersToRemove: backendConfig.request_headers_to_remove || [],
					responseHeadersToAdd: backendConfig.response_headers_to_add || [],
					responseHeadersToRemove: backendConfig.response_headers_to_remove || []
				};
			} else if (data.config.type === 'jwt_auth') {
				// JWT config is already in correct format from backend
				jwtAuthConfig = data.config.config;
			} else if (data.config.type === 'local_rate_limit') {
				// LocalRateLimit config is already in correct format from backend
				localRateLimitConfig = data.config.config;
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load filter';
		} finally {
			isLoading = false;
		}
	}

	// Format filter type for display (convert snake_case to Title Case)
	function formatFilterType(type: string): string {
		return type.split('_').map(word => word.charAt(0).toUpperCase() + word.slice(1)).join(' ');
	}

	// Build the filter config based on filter type
	// Uses tagged enum format with snake_case to match backend Rust serialization
	function buildFilterConfig(): FilterConfig {
		if (filter?.filterType === 'jwt_auth') {
			return {
				type: 'jwt_auth',
				config: jwtAuthConfig
			};
		}

		if (filter?.filterType === 'local_rate_limit') {
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
		if (filter?.filterType === 'jwt_auth') {
			return validateJwtAuthConfig();
		}

		if (filter?.filterType === 'local_rate_limit') {
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
		const headersToAdd = [
			...(config.requestHeadersToAdd || []),
			...(config.responseHeadersToAdd || [])
		];
		for (const header of headersToAdd) {
			if (!header.key.trim()) {
				return 'Header name cannot be empty';
			}
			if (!header.value.trim()) {
				return `Value for header "${header.key}" cannot be empty`;
			}
		}

		// Validate headers to remove have values
		const headersToRemove = [
			...(config.requestHeadersToRemove || []),
			...(config.responseHeadersToRemove || [])
		];
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
			await apiClient.updateFilter(getFilterId(), {
				name: filterName.trim(),
				description: filterDescription.trim() || undefined,
				config: buildFilterConfig()
			});
			goto('/filters');
		} catch (e) {
			console.error('Update filter failed:', e);
			error = e instanceof Error ? e.message : 'Failed to update filter';
		} finally {
			isSubmitting = false;
		}
	}

	// Handle delete
	async function handleDelete() {
		if (
			!confirm(
				`Are you sure you want to delete the filter "${filter?.name}"? This action cannot be undone.`
			)
		) {
			return;
		}

		try {
			await apiClient.deleteFilter(getFilterId());
			goto('/filters');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to delete filter';
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

	// Format date
	function formatDate(dateStr: string): string {
		const date = new Date(dateStr);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}
</script>

<div class="max-w-4xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
	{#if isLoading}
		<!-- Loading State -->
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading filter...</span>
			</div>
		</div>
	{:else if error && !filter}
		<!-- Error State (filter not found) -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
			<Button onclick={handleCancel} variant="secondary" size="sm">Back to Filters</Button>
		</div>
	{:else if filter}
		<!-- Page Header with Back Button -->
		<div class="mb-6">
			<div class="flex items-center gap-4 mb-2">
				<button
					onclick={handleCancel}
					class="p-2 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded-md transition-colors"
				>
					<ArrowLeft class="w-5 h-5" />
				</button>
				<div class="flex-1">
					<div class="flex items-center gap-3">
						<h1 class="text-2xl font-bold text-gray-900">{filter.name}</h1>
						<Badge variant="blue">{formatFilterType(filter.filterType)}</Badge>
						<span class="px-2 py-0.5 rounded text-xs font-medium bg-gray-100 text-gray-600">
							v{filter.version}
						</span>
					</div>
					{#if filter.description}
						<p class="text-sm text-gray-600 mt-1">{filter.description}</p>
					{/if}
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
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Description</label>
					<textarea
						bind:value={filterDescription}
						rows="2"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					></textarea>
				</div>

				<div class="grid grid-cols-2 gap-4">
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Team</label>
						<input
							type="text"
							value={filter.team}
							disabled
							class="w-full px-3 py-2 border border-gray-200 rounded-md bg-gray-50 text-gray-500"
						/>
					</div>
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Filter Type</label>
						<input
							type="text"
							value={formatFilterType(filter.filterType)}
							disabled
							class="w-full px-3 py-2 border border-gray-200 rounded-md bg-gray-50 text-gray-500"
						/>
						<p class="text-xs text-gray-500 mt-1">
							Filter type cannot be changed after creation
						</p>
					</div>
				</div>
			</div>
		</div>

		<!-- Attachment Points Info -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<div class="flex items-center justify-between mb-4">
				<h2 class="text-lg font-semibold text-gray-900">Attachment Points</h2>
				<div class="flex gap-1">
					{#if filter.filterType === 'jwt_auth' || filter.filterType === 'rate_limit'}
						<Badge variant="blue">Routes</Badge>
						<Badge variant="blue">Listeners</Badge>
					{:else}
						<Badge variant="blue">Routes only</Badge>
					{/if}
				</div>
			</div>
			<p class="text-sm text-gray-500">
				{#if filter.filterType === 'jwt_auth'}
					JWT Auth filters can attach to routes or listeners (L7 HTTP filter)
				{:else if filter.filterType === 'rate_limit'}
					Rate Limit filters can attach to routes or listeners (L7 HTTP filter)
				{:else}
					HeaderMutation filters can only attach to routes (L7 HTTP filter)
				{/if}
			</p>
		</div>

		<!-- Configuration -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			{#if filter.filterType === 'jwt_auth'}
				<h2 class="text-lg font-semibold text-gray-900 mb-4">JWT Authentication Configuration</h2>
				<JwtAuthConfigForm config={jwtAuthConfig} onConfigChange={handleJwtConfigChange} />
			{:else if filter.filterType === 'rate_limit'}
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Rate Limit Configuration</h2>
				<LocalRateLimitForm config={localRateLimitConfig} onConfigChange={handleLocalRateLimitConfigChange} />
			{:else}
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Header Mutation Configuration</h2>
				<HeaderMutationConfigForm
					config={headerMutationConfig}
					onConfigChange={handleConfigChange}
				/>
			{/if}
		</div>

		<!-- Metadata -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Metadata</h2>
			<div class="grid grid-cols-2 gap-4 text-sm">
				<div>
					<span class="text-gray-500">Created:</span>
					<span class="text-gray-900 ml-2">{formatDate(filter.createdAt)}</span>
				</div>
				<div>
					<span class="text-gray-500">Updated:</span>
					<span class="text-gray-900 ml-2">{formatDate(filter.updatedAt)}</span>
				</div>
				<div>
					<span class="text-gray-500">Version:</span>
					<span class="text-gray-900 ml-2">{filter.version}</span>
				</div>
				<div>
					<span class="text-gray-500">Source:</span>
					<span class="text-gray-900 ml-2 capitalize">{filter.source}</span>
				</div>
			</div>
		</div>

		<!-- Action Buttons -->
		<div class="flex justify-between">
			<Button onclick={handleDelete} variant="danger" disabled={isSubmitting}>
				Delete Filter
			</Button>
			<div class="flex gap-3">
				<Button onclick={handleCancel} variant="secondary" disabled={isSubmitting}>
					Cancel
				</Button>
				<Button onclick={handleSubmit} variant="primary" disabled={isSubmitting}>
					{isSubmitting ? 'Saving...' : 'Save Changes'}
				</Button>
			</div>
		</div>
	{/if}
</div>
