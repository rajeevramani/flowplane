<script lang="ts">
	import type { ExtAuthzConfig, ExtAuthzHeaderKeyValue } from '$lib/api/types';
	import { ExtAuthzConfigSchema } from '$lib/schemas/filter-configs';
	import { Info, Plus, Trash2, Settings, ChevronRight } from 'lucide-svelte';

	interface Props {
		config: ExtAuthzConfig;
		onConfigChange: (config: ExtAuthzConfig) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Service type
	let serviceType = $state<'grpc' | 'http'>(config.service?.type ?? 'grpc');

	// gRPC settings
	let grpcClusterName = $state(config.service?.target_uri ?? '');
	let grpcTimeoutMs = $state<number>(config.service?.timeout_ms ?? 200);
	let grpcInitialMetadata = $state<ExtAuthzHeaderKeyValue[]>(
		config.service?.initial_metadata ?? []
	);

	// HTTP settings
	let httpUri = $state(config.service?.server_uri?.uri ?? '');
	let httpCluster = $state(config.service?.server_uri?.cluster ?? '');
	let httpTimeoutMs = $state<number>(config.service?.server_uri?.timeout_ms ?? 200);
	let httpPathPrefix = $state(config.service?.path_prefix ?? '');
	let httpHeadersToAdd = $state<ExtAuthzHeaderKeyValue[]>(
		config.service?.headers_to_add ?? []
	);

	// Failure handling
	let failureModeAllow = $state(config.failure_mode_allow ?? false);
	let statusOnError = $state<number | undefined>(config.status_on_error);
	let clearRouteCache = $state(config.clear_route_cache ?? false);

	// Request body
	let enableRequestBody = $state(config.with_request_body !== undefined);
	let maxRequestBytes = $state<number>(config.with_request_body?.max_request_bytes ?? 8192);
	let allowPartialMessage = $state(config.with_request_body?.allow_partial_message ?? false);
	let packAsBytes = $state(config.with_request_body?.pack_as_bytes ?? false);

	// Request config
	let showRequestConfig = $state(false);
	let allowedRequestHeaders = $state<string[]>(
		config.service?.authorization_request?.allowed_headers ?? []
	);
	let requestHeadersToAdd = $state<ExtAuthzHeaderKeyValue[]>(
		config.service?.authorization_request?.headers_to_add ?? []
	);

	// Response config
	let showResponseConfig = $state(false);
	let allowedUpstreamHeaders = $state<string[]>(
		config.service?.authorization_response?.allowed_upstream_headers ?? []
	);
	let allowedClientHeaders = $state<string[]>(
		config.service?.authorization_response?.allowed_client_headers ?? []
	);
	let allowedClientHeadersOnSuccess = $state<string[]>(
		config.service?.authorization_response?.allowed_client_headers_on_success ?? []
	);

	// Advanced
	let showAdvanced = $state(false);
	let statPrefix = $state(config.stat_prefix ?? '');
	let includePeerCertificate = $state(config.include_peer_certificate ?? false);

	// Input fields
	let newRequestHeader = $state('');
	let newUpstreamHeader = $state('');
	let newClientHeader = $state('');
	let newClientHeaderOnSuccess = $state('');

	// Validation errors
	let validationErrors = $state<string[]>([]);

	function updateParent() {
		const service: ExtAuthzConfig['service'] = { type: serviceType };

		if (serviceType === 'grpc') {
			if (grpcClusterName) service.target_uri = grpcClusterName;
			if (grpcTimeoutMs) service.timeout_ms = grpcTimeoutMs;
			if (grpcInitialMetadata.length > 0) service.initial_metadata = grpcInitialMetadata;
		} else {
			if (httpUri || httpCluster) {
				service.server_uri = {};
				if (httpUri) service.server_uri.uri = httpUri;
				if (httpCluster) service.server_uri.cluster = httpCluster;
				if (httpTimeoutMs) service.server_uri.timeout_ms = httpTimeoutMs;
			}
			if (httpPathPrefix) service.path_prefix = httpPathPrefix;
			if (httpHeadersToAdd.length > 0) service.headers_to_add = httpHeadersToAdd;
		}

		// Request/response config
		if (allowedRequestHeaders.length > 0 || requestHeadersToAdd.length > 0) {
			service.authorization_request = {};
			if (allowedRequestHeaders.length > 0) service.authorization_request.allowed_headers = allowedRequestHeaders;
			if (requestHeadersToAdd.length > 0) service.authorization_request.headers_to_add = requestHeadersToAdd;
		}
		if (allowedUpstreamHeaders.length > 0 || allowedClientHeaders.length > 0 || allowedClientHeadersOnSuccess.length > 0) {
			service.authorization_response = {};
			if (allowedUpstreamHeaders.length > 0) service.authorization_response.allowed_upstream_headers = allowedUpstreamHeaders;
			if (allowedClientHeaders.length > 0) service.authorization_response.allowed_client_headers = allowedClientHeaders;
			if (allowedClientHeadersOnSuccess.length > 0) service.authorization_response.allowed_client_headers_on_success = allowedClientHeadersOnSuccess;
		}

		const cfg: ExtAuthzConfig = { service };
		if (failureModeAllow) cfg.failure_mode_allow = true;
		if (clearRouteCache) cfg.clear_route_cache = true;
		if (statusOnError !== undefined && statusOnError >= 100) cfg.status_on_error = statusOnError;
		if (enableRequestBody) {
			cfg.with_request_body = {
				max_request_bytes: maxRequestBytes,
				allow_partial_message: allowPartialMessage || undefined,
				pack_as_bytes: packAsBytes || undefined
			};
		}
		if (statPrefix) cfg.stat_prefix = statPrefix;
		if (includePeerCertificate) cfg.include_peer_certificate = true;

		const result = ExtAuthzConfigSchema.safeParse(cfg);
		validationErrors = result.success
			? []
			: result.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`);

		onConfigChange(cfg);
	}

	// Header list helpers
	function addToStringList(list: string[], value: string, setter: (v: string[]) => void, inputSetter: (v: string) => void) {
		const trimmed = value.trim();
		if (trimmed && !list.includes(trimmed)) {
			setter([...list, trimmed]);
			inputSetter('');
			updateParent();
		}
	}

	function removeFromStringList(list: string[], item: string, setter: (v: string[]) => void) {
		setter(list.filter((h) => h !== item));
		updateParent();
	}

	function addKeyValue(list: ExtAuthzHeaderKeyValue[], setter: (v: ExtAuthzHeaderKeyValue[]) => void) {
		setter([...list, { key: '', value: '' }]);
	}

	function removeKeyValue(list: ExtAuthzHeaderKeyValue[], index: number, setter: (v: ExtAuthzHeaderKeyValue[]) => void) {
		setter(list.filter((_, i) => i !== index));
		updateParent();
	}

	function updateKeyValue(list: ExtAuthzHeaderKeyValue[], index: number, field: 'key' | 'value', val: string, setter: (v: ExtAuthzHeaderKeyValue[]) => void) {
		setter(list.map((item, i) => (i === index ? { ...item, [field]: val } : item)));
		updateParent();
	}

	function handleKeydown(event: KeyboardEvent, addFn: () => void) {
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
				<p class="font-medium">External Authorization (ext_authz)</p>
				<p class="mt-1">
					Delegates access control decisions to an external authorization service.
					Supports both gRPC and HTTP service types. When the authz service is unreachable,
					behavior is controlled by the failure mode setting.
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

	<!-- Service Configuration -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">
			Service Configuration <span class="text-red-500">*</span>
		</h3>

		<!-- Service Type Toggle -->
		<div class="mb-4">
			<label class="block text-sm font-medium text-gray-700 mb-2">Service Type</label>
			<div class="flex gap-2">
				<button
					type="button"
					onclick={() => { serviceType = 'grpc'; updateParent(); }}
					class="px-4 py-2 text-sm rounded-md border transition-colors {serviceType === 'grpc'
						? 'bg-blue-100 border-blue-300 text-blue-800 font-medium'
						: 'bg-white border-gray-300 text-gray-600 hover:border-gray-400'}"
				>
					gRPC
				</button>
				<button
					type="button"
					onclick={() => { serviceType = 'http'; updateParent(); }}
					class="px-4 py-2 text-sm rounded-md border transition-colors {serviceType === 'http'
						? 'bg-blue-100 border-blue-300 text-blue-800 font-medium'
						: 'bg-white border-gray-300 text-gray-600 hover:border-gray-400'}"
				>
					HTTP
				</button>
			</div>
		</div>

		{#if serviceType === 'grpc'}
			<!-- gRPC Service Settings -->
			<div class="space-y-4">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Target Cluster <span class="text-red-500">*</span>
					</label>
					<input
						type="text"
						bind:value={grpcClusterName}
						oninput={updateParent}
						placeholder="e.g., ext-authz-cluster"
						class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Name of the cluster hosting the gRPC authorization service
					</p>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Timeout (ms)
					</label>
					<input
						type="number"
						bind:value={grpcTimeoutMs}
						oninput={updateParent}
						min="1"
						placeholder="200"
						class="w-32 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
				</div>

				<!-- Initial Metadata -->
				<div>
					<div class="flex items-center justify-between mb-2">
						<label class="block text-sm font-medium text-gray-700">Initial Metadata</label>
						<button
							type="button"
							onclick={() => addKeyValue(grpcInitialMetadata, (v) => (grpcInitialMetadata = v))}
							class="inline-flex items-center gap-1 px-2 py-1 text-xs font-medium text-blue-700 bg-blue-50 border border-blue-200 rounded hover:bg-blue-100 transition-colors"
						>
							<Plus class="w-3 h-3" />
							Add
						</button>
					</div>
					{#each grpcInitialMetadata as meta, index}
						<div class="flex items-center gap-2 mb-2">
							<input
								type="text"
								value={meta.key}
								oninput={(e) => updateKeyValue(grpcInitialMetadata, index, 'key', (e.target as HTMLInputElement).value, (v) => (grpcInitialMetadata = v))}
								placeholder="Key"
								class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							<input
								type="text"
								value={meta.value}
								oninput={(e) => updateKeyValue(grpcInitialMetadata, index, 'value', (e.target as HTMLInputElement).value, (v) => (grpcInitialMetadata = v))}
								placeholder="Value"
								class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							<button
								type="button"
								onclick={() => removeKeyValue(grpcInitialMetadata, index, (v) => (grpcInitialMetadata = v))}
								class="p-1.5 text-gray-400 hover:text-red-600 transition-colors"
							>
								<Trash2 class="w-4 h-4" />
							</button>
						</div>
					{/each}
				</div>
			</div>
		{:else}
			<!-- HTTP Service Settings -->
			<div class="space-y-4">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Server URI <span class="text-red-500">*</span>
					</label>
					<input
						type="text"
						bind:value={httpUri}
						oninput={updateParent}
						placeholder="e.g., http://authz-service:8080/check"
						class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Cluster
					</label>
					<input
						type="text"
						bind:value={httpCluster}
						oninput={updateParent}
						placeholder="e.g., ext-authz-cluster"
						class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Cluster name for the upstream authorization service
					</p>
				</div>

				<div class="flex gap-4">
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Timeout (ms)
						</label>
						<input
							type="number"
							bind:value={httpTimeoutMs}
							oninput={updateParent}
							min="1"
							placeholder="200"
							class="w-32 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
					</div>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Path Prefix
					</label>
					<input
						type="text"
						bind:value={httpPathPrefix}
						oninput={updateParent}
						placeholder="e.g., /auth"
						class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Prepended to the original request path for authorization requests
					</p>
				</div>

				<!-- Headers to Add -->
				<div>
					<div class="flex items-center justify-between mb-2">
						<label class="block text-sm font-medium text-gray-700">Headers to Add</label>
						<button
							type="button"
							onclick={() => addKeyValue(httpHeadersToAdd, (v) => (httpHeadersToAdd = v))}
							class="inline-flex items-center gap-1 px-2 py-1 text-xs font-medium text-blue-700 bg-blue-50 border border-blue-200 rounded hover:bg-blue-100 transition-colors"
						>
							<Plus class="w-3 h-3" />
							Add
						</button>
					</div>
					{#each httpHeadersToAdd as header, index}
						<div class="flex items-center gap-2 mb-2">
							<input
								type="text"
								value={header.key}
								oninput={(e) => updateKeyValue(httpHeadersToAdd, index, 'key', (e.target as HTMLInputElement).value, (v) => (httpHeadersToAdd = v))}
								placeholder="Header name"
								class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							<input
								type="text"
								value={header.value}
								oninput={(e) => updateKeyValue(httpHeadersToAdd, index, 'value', (e.target as HTMLInputElement).value, (v) => (httpHeadersToAdd = v))}
								placeholder="Header value"
								class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							<button
								type="button"
								onclick={() => removeKeyValue(httpHeadersToAdd, index, (v) => (httpHeadersToAdd = v))}
								class="p-1.5 text-gray-400 hover:text-red-600 transition-colors"
							>
								<Trash2 class="w-4 h-4" />
							</button>
						</div>
					{/each}
				</div>
			</div>
		{/if}
	</div>

	<!-- Failure Handling -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Failure Handling</h3>
		<div class="space-y-4">
			<label class="flex items-center gap-3">
				<input
					type="checkbox"
					bind:checked={failureModeAllow}
					onchange={updateParent}
					class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
				/>
				<div>
					<span class="text-sm font-medium text-gray-700">Failure Mode Allow</span>
					<p class="text-xs text-gray-500">
						Allow requests when the authorization service is unavailable (default: deny)
					</p>
				</div>
			</label>

			{#if failureModeAllow}
				<div class="rounded-lg border border-amber-100 bg-amber-50 p-2">
					<p class="text-xs text-amber-700">
						Warning: Enabling failure mode allow means requests will be permitted when the authz service is down.
					</p>
				</div>
			{/if}

			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Status on Error
				</label>
				<input
					type="number"
					bind:value={statusOnError}
					oninput={updateParent}
					min="100"
					max="599"
					placeholder="Default (403)"
					class="w-32 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
				<p class="text-xs text-gray-500 mt-1">
					HTTP status code returned when the authz service returns an error
				</p>
			</div>

			<label class="flex items-center gap-3">
				<input
					type="checkbox"
					bind:checked={clearRouteCache}
					onchange={updateParent}
					class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
				/>
				<div>
					<span class="text-sm font-medium text-gray-700">Clear Route Cache</span>
					<p class="text-xs text-gray-500">
						Clear the route cache on successful authorization (allows authz to modify routing)
					</p>
				</div>
			</label>
		</div>
	</div>

	<!-- Request Body -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Request Body</h3>
		<div class="space-y-4">
			<label class="flex items-center gap-3">
				<input
					type="checkbox"
					bind:checked={enableRequestBody}
					onchange={updateParent}
					class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
				/>
				<div>
					<span class="text-sm font-medium text-gray-700">Buffer Request Body</span>
					<p class="text-xs text-gray-500">
						Include request body in authorization requests
					</p>
				</div>
			</label>

			{#if enableRequestBody}
				<div class="ml-7 space-y-4">
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Max Request Bytes
						</label>
						<input
							type="number"
							bind:value={maxRequestBytes}
							oninput={updateParent}
							min="0"
							class="w-32 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<p class="text-xs text-gray-500 mt-1">
							Maximum bytes to buffer from request body
						</p>
					</div>

					<label class="flex items-center gap-3">
						<input
							type="checkbox"
							bind:checked={allowPartialMessage}
							onchange={updateParent}
							class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
						/>
						<div>
							<span class="text-sm text-gray-700">Allow Partial Message</span>
							<p class="text-xs text-gray-500">Send partial body if max bytes exceeded</p>
						</div>
					</label>

					<label class="flex items-center gap-3">
						<input
							type="checkbox"
							bind:checked={packAsBytes}
							onchange={updateParent}
							class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
						/>
						<div>
							<span class="text-sm text-gray-700">Pack as Bytes</span>
							<p class="text-xs text-gray-500">Pack body as raw bytes instead of UTF-8 string</p>
						</div>
					</label>
				</div>
			{/if}
		</div>
	</div>

	<!-- Request Configuration (collapsible) -->
	<div>
		<button
			type="button"
			onclick={() => (showRequestConfig = !showRequestConfig)}
			class="flex items-center gap-2 text-sm font-medium text-gray-600 hover:text-gray-900"
		>
			<ChevronRight class="w-4 h-4 transition-transform {showRequestConfig ? 'rotate-90' : ''}" />
			Request Configuration
		</button>

		{#if showRequestConfig}
			<div class="mt-4 space-y-4 pl-6 border-l-2 border-gray-200">
				<!-- Allowed Request Headers -->
				<div>
					<h4 class="text-sm font-medium text-gray-700 mb-2">Allowed Headers from Original Request</h4>
					<div class="flex items-center gap-2 mb-2">
						<input
							type="text"
							bind:value={newRequestHeader}
							onkeydown={(e) => handleKeydown(e, () => addToStringList(allowedRequestHeaders, newRequestHeader, (v) => (allowedRequestHeaders = v), (v) => (newRequestHeader = v)))}
							placeholder="e.g., Authorization"
							class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<button
							type="button"
							onclick={() => addToStringList(allowedRequestHeaders, newRequestHeader, (v) => (allowedRequestHeaders = v), (v) => (newRequestHeader = v))}
							class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
						>
							<Plus class="w-4 h-4" />
						</button>
					</div>
					{#if allowedRequestHeaders.length > 0}
						<div class="flex flex-wrap gap-1.5">
							{#each allowedRequestHeaders as header}
								<span class="inline-flex items-center gap-1 px-2 py-1 bg-gray-100 text-gray-700 text-xs rounded-md">
									{header}
									<button
										type="button"
										onclick={() => removeFromStringList(allowedRequestHeaders, header, (v) => (allowedRequestHeaders = v))}
										class="text-gray-400 hover:text-red-500"
									>
										<Trash2 class="w-3 h-3" />
									</button>
								</span>
							{/each}
						</div>
					{/if}
					<p class="text-xs text-gray-500 mt-2">Headers from the original request to include in the authz request</p>
				</div>

				<!-- Additional Headers to Add -->
				<div>
					<div class="flex items-center justify-between mb-2">
						<label class="block text-sm font-medium text-gray-700">Additional Headers</label>
						<button
							type="button"
							onclick={() => addKeyValue(requestHeadersToAdd, (v) => (requestHeadersToAdd = v))}
							class="inline-flex items-center gap-1 px-2 py-1 text-xs font-medium text-blue-700 bg-blue-50 border border-blue-200 rounded hover:bg-blue-100 transition-colors"
						>
							<Plus class="w-3 h-3" />
							Add
						</button>
					</div>
					{#each requestHeadersToAdd as header, index}
						<div class="flex items-center gap-2 mb-2">
							<input
								type="text"
								value={header.key}
								oninput={(e) => updateKeyValue(requestHeadersToAdd, index, 'key', (e.target as HTMLInputElement).value, (v) => (requestHeadersToAdd = v))}
								placeholder="Header name"
								class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							<input
								type="text"
								value={header.value}
								oninput={(e) => updateKeyValue(requestHeadersToAdd, index, 'value', (e.target as HTMLInputElement).value, (v) => (requestHeadersToAdd = v))}
								placeholder="Header value"
								class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							<button
								type="button"
								onclick={() => removeKeyValue(requestHeadersToAdd, index, (v) => (requestHeadersToAdd = v))}
								class="p-1.5 text-gray-400 hover:text-red-600 transition-colors"
							>
								<Trash2 class="w-4 h-4" />
							</button>
						</div>
					{/each}
				</div>
			</div>
		{/if}
	</div>

	<!-- Response Configuration (collapsible) -->
	<div>
		<button
			type="button"
			onclick={() => (showResponseConfig = !showResponseConfig)}
			class="flex items-center gap-2 text-sm font-medium text-gray-600 hover:text-gray-900"
		>
			<ChevronRight class="w-4 h-4 transition-transform {showResponseConfig ? 'rotate-90' : ''}" />
			Response Configuration
		</button>

		{#if showResponseConfig}
			<div class="mt-4 space-y-4 pl-6 border-l-2 border-gray-200">
				<!-- Allowed Upstream Headers -->
				<div>
					<h4 class="text-sm font-medium text-gray-700 mb-2">Allowed Upstream Headers</h4>
					<div class="flex items-center gap-2 mb-2">
						<input
							type="text"
							bind:value={newUpstreamHeader}
							onkeydown={(e) => handleKeydown(e, () => addToStringList(allowedUpstreamHeaders, newUpstreamHeader, (v) => (allowedUpstreamHeaders = v), (v) => (newUpstreamHeader = v)))}
							placeholder="e.g., X-Auth-User"
							class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<button
							type="button"
							onclick={() => addToStringList(allowedUpstreamHeaders, newUpstreamHeader, (v) => (allowedUpstreamHeaders = v), (v) => (newUpstreamHeader = v))}
							class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
						>
							<Plus class="w-4 h-4" />
						</button>
					</div>
					{#if allowedUpstreamHeaders.length > 0}
						<div class="flex flex-wrap gap-1.5">
							{#each allowedUpstreamHeaders as header}
								<span class="inline-flex items-center gap-1 px-2 py-1 bg-gray-100 text-gray-700 text-xs rounded-md">
									{header}
									<button
										type="button"
										onclick={() => removeFromStringList(allowedUpstreamHeaders, header, (v) => (allowedUpstreamHeaders = v))}
										class="text-gray-400 hover:text-red-500"
									>
										<Trash2 class="w-3 h-3" />
									</button>
								</span>
							{/each}
						</div>
					{/if}
					<p class="text-xs text-gray-500 mt-2">Headers from authz response to add to the upstream request</p>
				</div>

				<!-- Allowed Client Headers (denial) -->
				<div>
					<h4 class="text-sm font-medium text-gray-700 mb-2">Allowed Client Headers (on denial)</h4>
					<div class="flex items-center gap-2 mb-2">
						<input
							type="text"
							bind:value={newClientHeader}
							onkeydown={(e) => handleKeydown(e, () => addToStringList(allowedClientHeaders, newClientHeader, (v) => (allowedClientHeaders = v), (v) => (newClientHeader = v)))}
							placeholder="e.g., X-Error-Reason"
							class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<button
							type="button"
							onclick={() => addToStringList(allowedClientHeaders, newClientHeader, (v) => (allowedClientHeaders = v), (v) => (newClientHeader = v))}
							class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
						>
							<Plus class="w-4 h-4" />
						</button>
					</div>
					{#if allowedClientHeaders.length > 0}
						<div class="flex flex-wrap gap-1.5">
							{#each allowedClientHeaders as header}
								<span class="inline-flex items-center gap-1 px-2 py-1 bg-gray-100 text-gray-700 text-xs rounded-md">
									{header}
									<button
										type="button"
										onclick={() => removeFromStringList(allowedClientHeaders, header, (v) => (allowedClientHeaders = v))}
										class="text-gray-400 hover:text-red-500"
									>
										<Trash2 class="w-3 h-3" />
									</button>
								</span>
							{/each}
						</div>
					{/if}
					<p class="text-xs text-gray-500 mt-2">Headers from authz response to include in denial responses</p>
				</div>

				<!-- Allowed Client Headers (success) -->
				<div>
					<h4 class="text-sm font-medium text-gray-700 mb-2">Allowed Client Headers (on success)</h4>
					<div class="flex items-center gap-2 mb-2">
						<input
							type="text"
							bind:value={newClientHeaderOnSuccess}
							onkeydown={(e) => handleKeydown(e, () => addToStringList(allowedClientHeadersOnSuccess, newClientHeaderOnSuccess, (v) => (allowedClientHeadersOnSuccess = v), (v) => (newClientHeaderOnSuccess = v)))}
							placeholder="e.g., X-Auth-Token"
							class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<button
							type="button"
							onclick={() => addToStringList(allowedClientHeadersOnSuccess, newClientHeaderOnSuccess, (v) => (allowedClientHeadersOnSuccess = v), (v) => (newClientHeaderOnSuccess = v))}
							class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
						>
							<Plus class="w-4 h-4" />
						</button>
					</div>
					{#if allowedClientHeadersOnSuccess.length > 0}
						<div class="flex flex-wrap gap-1.5">
							{#each allowedClientHeadersOnSuccess as header}
								<span class="inline-flex items-center gap-1 px-2 py-1 bg-gray-100 text-gray-700 text-xs rounded-md">
									{header}
									<button
										type="button"
										onclick={() => removeFromStringList(allowedClientHeadersOnSuccess, header, (v) => (allowedClientHeadersOnSuccess = v))}
										class="text-gray-400 hover:text-red-500"
									>
										<Trash2 class="w-3 h-3" />
									</button>
								</span>
							{/each}
						</div>
					{/if}
					<p class="text-xs text-gray-500 mt-2">Headers from authz response to include in success responses</p>
				</div>
			</div>
		{/if}
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
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Stats Prefix
					</label>
					<input
						type="text"
						bind:value={statPrefix}
						oninput={updateParent}
						placeholder="e.g., ext_authz"
						class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Prefix for statistics emitted by this filter
					</p>
				</div>

				<label class="flex items-center gap-3">
					<input
						type="checkbox"
						bind:checked={includePeerCertificate}
						onchange={updateParent}
						class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
					/>
					<div>
						<span class="text-sm font-medium text-gray-700">Include Peer Certificate</span>
						<p class="text-xs text-gray-500">
							Include client TLS certificate in authorization request metadata
						</p>
					</div>
				</label>
			</div>
		{/if}
	</div>
</div>
