<script lang="ts">
	import type {
		CorsConfig,
		CorsOriginMatcher,
		CorsMatchType,
		RuntimeFractionalPercentConfig
	} from '$lib/api/types';
	import { CorsConfigSchema } from '$lib/schemas/filter-configs';
	import { Info, Plus, Trash2, Settings, ChevronRight } from 'lucide-svelte';

	interface Props {
		config: CorsConfig;
		onConfigChange: (config: CorsConfig) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Initialize state from config
	let origins = $state<CorsOriginMatcher[]>(config.policy?.allow_origin ?? [{ type: 'exact', value: '' }]);
	let allowMethods = $state<string[]>(config.policy?.allow_methods ?? []);
	let allowHeaders = $state<string[]>(config.policy?.allow_headers ?? []);
	let exposeHeaders = $state<string[]>(config.policy?.expose_headers ?? []);
	let maxAge = $state<number | undefined>(config.policy?.max_age);
	let allowCredentials = $state(config.policy?.allow_credentials ?? false);
	let allowPrivateNetworkAccess = $state(config.policy?.allow_private_network_access ?? false);
	let forwardNotMatchingPreflights = $state(config.policy?.forward_not_matching_preflights ?? false);

	// Advanced
	let showAdvanced = $state(false);
	let filterEnabledActive = $state(config.policy?.filter_enabled !== undefined);
	let filterEnabledNumerator = $state(config.policy?.filter_enabled?.numerator ?? 100);
	let shadowEnabledActive = $state(config.policy?.shadow_enabled !== undefined);
	let shadowEnabledNumerator = $state(config.policy?.shadow_enabled?.numerator ?? 0);

	// New header/method input
	let newAllowHeader = $state('');
	let newExposeHeader = $state('');

	// Validation errors
	let validationErrors = $state<string[]>([]);

	const HTTP_METHODS = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'OPTIONS', 'HEAD', '*'];
	const MATCH_TYPES: { value: CorsMatchType; label: string }[] = [
		{ value: 'exact', label: 'Exact' },
		{ value: 'prefix', label: 'Prefix' },
		{ value: 'suffix', label: 'Suffix' },
		{ value: 'contains', label: 'Contains' },
		{ value: 'regex', label: 'Regex' }
	];

	function updateParent() {
		const policy: CorsConfig['policy'] = {
			allow_origin: origins.filter((o) => o.value.trim() !== '')
		};

		if (allowMethods.length > 0) policy.allow_methods = allowMethods;
		if (allowHeaders.length > 0) policy.allow_headers = allowHeaders;
		if (exposeHeaders.length > 0) policy.expose_headers = exposeHeaders;
		if (maxAge !== undefined && maxAge >= 0) policy.max_age = maxAge;
		if (allowCredentials) policy.allow_credentials = true;
		if (allowPrivateNetworkAccess) policy.allow_private_network_access = true;
		if (forwardNotMatchingPreflights) policy.forward_not_matching_preflights = true;

		if (filterEnabledActive) {
			policy.filter_enabled = {
				numerator: filterEnabledNumerator,
				denominator: 'hundred'
			};
		}
		if (shadowEnabledActive) {
			policy.shadow_enabled = {
				numerator: shadowEnabledNumerator,
				denominator: 'hundred'
			};
		}

		const result = CorsConfigSchema.safeParse({ policy });
		validationErrors = result.success
			? []
			: result.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`);

		onConfigChange({ policy });
	}

	function addOrigin() {
		origins = [...origins, { type: 'exact', value: '' }];
	}

	function removeOrigin(index: number) {
		origins = origins.filter((_, i) => i !== index);
		updateParent();
	}

	function updateOriginType(index: number, type: CorsMatchType) {
		origins = origins.map((o, i) => (i === index ? { ...o, type } : o));
		updateParent();
	}

	function updateOriginValue(index: number, value: string) {
		origins = origins.map((o, i) => (i === index ? { ...o, value } : o));
		updateParent();
	}

	function toggleMethod(method: string) {
		if (allowMethods.includes(method)) {
			allowMethods = allowMethods.filter((m) => m !== method);
		} else {
			allowMethods = [...allowMethods, method];
		}
		updateParent();
	}

	function addAllowHeader() {
		const trimmed = newAllowHeader.trim();
		if (trimmed && !allowHeaders.includes(trimmed)) {
			allowHeaders = [...allowHeaders, trimmed];
			newAllowHeader = '';
			updateParent();
		}
	}

	function removeAllowHeader(header: string) {
		allowHeaders = allowHeaders.filter((h) => h !== header);
		updateParent();
	}

	function addExposeHeader() {
		const trimmed = newExposeHeader.trim();
		if (trimmed && !exposeHeaders.includes(trimmed)) {
			exposeHeaders = [...exposeHeaders, trimmed];
			newExposeHeader = '';
			updateParent();
		}
	}

	function removeExposeHeader(header: string) {
		exposeHeaders = exposeHeaders.filter((h) => h !== header);
		updateParent();
	}

	function handleHeaderKeydown(event: KeyboardEvent, addFn: () => void) {
		if (event.key === 'Enter') {
			event.preventDefault();
			addFn();
		}
	}
</script>

<div class="space-y-6">
	<!-- Info Box -->
	<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
		<div class="flex items-start gap-2">
			<Info class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0" />
			<div class="text-xs text-blue-700">
				<p class="font-medium">CORS (Cross-Origin Resource Sharing)</p>
				<p class="mt-1">
					Controls which origins, methods, and headers are allowed for cross-origin
					browser requests. Preflight OPTIONS requests are handled by the filter directly
					and do not reach the upstream service.
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

	<!-- Origins Section -->
	<div class="border border-gray-200 rounded-lg p-4">
		<div class="flex items-center justify-between mb-3">
			<h3 class="text-sm font-medium text-gray-900">
				Allowed Origins <span class="text-red-500">*</span>
			</h3>
			<button
				type="button"
				onclick={addOrigin}
				class="inline-flex items-center gap-1 px-2 py-1 text-xs font-medium text-blue-700 bg-blue-50 border border-blue-200 rounded hover:bg-blue-100 transition-colors"
			>
				<Plus class="w-3 h-3" />
				Add Origin
			</button>
		</div>

		<div class="space-y-2">
			{#each origins as origin, index}
				<div class="flex items-center gap-2">
					<select
						value={origin.type}
						onchange={(e) => updateOriginType(index, (e.target as HTMLSelectElement).value as CorsMatchType)}
						class="w-28 px-2 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					>
						{#each MATCH_TYPES as mt}
							<option value={mt.value}>{mt.label}</option>
						{/each}
					</select>
					<input
						type="text"
						value={origin.value}
						oninput={(e) => updateOriginValue(index, (e.target as HTMLInputElement).value)}
						placeholder={origin.type === 'regex' ? '^https://.*\\.example\\.com$' : 'https://example.com'}
						class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					{#if origins.length > 1}
						<button
							type="button"
							onclick={() => removeOrigin(index)}
							class="p-1.5 text-gray-400 hover:text-red-600 transition-colors"
						>
							<Trash2 class="w-4 h-4" />
						</button>
					{/if}
				</div>
			{/each}
		</div>
		<p class="text-xs text-gray-500 mt-2">
			Origins that are allowed to make cross-origin requests
		</p>
	</div>

	<!-- Methods Section -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Allowed Methods</h3>
		<div class="flex flex-wrap gap-2">
			{#each HTTP_METHODS as method}
				<button
					type="button"
					onclick={() => toggleMethod(method)}
					class="px-3 py-1.5 text-sm rounded-md border transition-colors {allowMethods.includes(method)
						? 'bg-blue-100 border-blue-300 text-blue-800 font-medium'
						: 'bg-white border-gray-300 text-gray-600 hover:border-gray-400'}"
				>
					{method}
				</button>
			{/each}
		</div>
		<p class="text-xs text-gray-500 mt-2">
			HTTP methods allowed for CORS requests. Use * to allow all methods.
		</p>
	</div>

	<!-- Headers Section -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Allow Headers</h3>
		<div class="flex items-center gap-2 mb-2">
			<input
				type="text"
				bind:value={newAllowHeader}
				onkeydown={(e) => handleHeaderKeydown(e, addAllowHeader)}
				placeholder="e.g., Authorization, Content-Type"
				class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
			/>
			<button
				type="button"
				onclick={addAllowHeader}
				class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
			>
				<Plus class="w-4 h-4" />
			</button>
		</div>
		{#if allowHeaders.length > 0}
			<div class="flex flex-wrap gap-1.5">
				{#each allowHeaders as header}
					<span class="inline-flex items-center gap-1 px-2 py-1 bg-gray-100 text-gray-700 text-xs rounded-md">
						{header}
						<button
							type="button"
							onclick={() => removeAllowHeader(header)}
							class="text-gray-400 hover:text-red-500"
						>
							<Trash2 class="w-3 h-3" />
						</button>
					</span>
				{/each}
			</div>
		{/if}
		<p class="text-xs text-gray-500 mt-2">Headers allowed in CORS requests (press Enter to add)</p>
	</div>

	<!-- Expose Headers Section -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Expose Headers</h3>
		<div class="flex items-center gap-2 mb-2">
			<input
				type="text"
				bind:value={newExposeHeader}
				onkeydown={(e) => handleHeaderKeydown(e, addExposeHeader)}
				placeholder="e.g., X-Request-Id, X-RateLimit-Remaining"
				class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
			/>
			<button
				type="button"
				onclick={addExposeHeader}
				class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
			>
				<Plus class="w-4 h-4" />
			</button>
		</div>
		{#if exposeHeaders.length > 0}
			<div class="flex flex-wrap gap-1.5">
				{#each exposeHeaders as header}
					<span class="inline-flex items-center gap-1 px-2 py-1 bg-gray-100 text-gray-700 text-xs rounded-md">
						{header}
						<button
							type="button"
							onclick={() => removeExposeHeader(header)}
							class="text-gray-400 hover:text-red-500"
						>
							<Trash2 class="w-3 h-3" />
						</button>
					</span>
				{/each}
			</div>
		{/if}
		<p class="text-xs text-gray-500 mt-2">Headers exposed to the browser in responses (press Enter to add)</p>
	</div>

	<!-- Credentials & Caching -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Credentials & Caching</h3>
		<div class="space-y-4">
			<label class="flex items-center gap-3">
				<input
					type="checkbox"
					bind:checked={allowCredentials}
					onchange={updateParent}
					class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
				/>
				<div>
					<span class="text-sm font-medium text-gray-700">Allow Credentials</span>
					<p class="text-xs text-gray-500">
						Allow cookies, authorization headers, and TLS client certificates
					</p>
				</div>
			</label>

			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Max Age (seconds)
				</label>
				<input
					type="number"
					bind:value={maxAge}
					oninput={updateParent}
					min="0"
					placeholder="Browser default"
					class="w-40 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
				<p class="text-xs text-gray-500 mt-1">
					How long browsers should cache preflight responses
				</p>
			</div>
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
				<!-- Filter Enabled -->
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
							<p class="text-xs text-gray-500">Enforce CORS for a percentage of requests</p>
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

				<!-- Shadow Enabled -->
				<div class="space-y-2">
					<label class="flex items-center gap-3">
						<input
							type="checkbox"
							bind:checked={shadowEnabledActive}
							onchange={updateParent}
							class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
						/>
						<div>
							<span class="text-sm font-medium text-gray-700">Shadow Mode</span>
							<p class="text-xs text-gray-500">Evaluate policy but don't enforce (for testing)</p>
						</div>
					</label>
					{#if shadowEnabledActive}
						<div class="ml-7 flex items-center gap-2">
							<input
								type="number"
								min="0"
								max="100"
								bind:value={shadowEnabledNumerator}
								oninput={updateParent}
								class="w-20 px-2 py-1 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							<span class="text-sm text-gray-600">% shadowed</span>
						</div>
					{/if}
				</div>

				<!-- Private Network Access -->
				<label class="flex items-center gap-3">
					<input
						type="checkbox"
						bind:checked={allowPrivateNetworkAccess}
						onchange={updateParent}
						class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
					/>
					<div>
						<span class="text-sm font-medium text-gray-700">Allow Private Network Access</span>
						<p class="text-xs text-gray-500">Allow requests targeting a more private network</p>
					</div>
				</label>

				<!-- Forward Non-matching Preflights -->
				<label class="flex items-center gap-3">
					<input
						type="checkbox"
						bind:checked={forwardNotMatchingPreflights}
						onchange={updateParent}
						class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
					/>
					<div>
						<span class="text-sm font-medium text-gray-700">Forward Non-matching Preflights</span>
						<p class="text-xs text-gray-500">Forward preflight requests that don't match configured origins</p>
					</div>
				</label>
			</div>
		{/if}
	</div>
</div>
