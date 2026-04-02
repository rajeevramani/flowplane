<script lang="ts">
	import type { OAuth2Config, PassThroughMatcher, OAuth2AuthType } from '$lib/api/types';
	import { OAuth2ConfigSchema } from '$lib/schemas/filter-configs';
	import { Info, Plus, Trash2, Settings, ChevronRight } from 'lucide-svelte';

	interface Props {
		config: OAuth2Config;
		onConfigChange: (config: OAuth2Config) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Token endpoint
	let tokenUri = $state(config.token_endpoint?.uri ?? '');
	let tokenCluster = $state(config.token_endpoint?.cluster ?? '');
	let tokenTimeoutMs = $state<number>(config.token_endpoint?.timeout_ms ?? 5000);

	// Authorization
	let authorizationEndpoint = $state(config.authorization_endpoint ?? '');

	// Credentials
	let clientId = $state(config.credentials?.client_id ?? '');
	let tokenSecretName = $state(config.credentials?.token_secret?.name ?? 'oauth2-token-secret');
	let cookieDomain = $state(config.credentials?.cookie_domain ?? '');

	// Redirect
	let redirectUri = $state(config.redirect_uri ?? '');
	let redirectPath = $state(config.redirect_path ?? '/oauth2/callback');
	let signoutPath = $state(config.signout_path ?? '');

	// Scopes
	let authScopes = $state<string[]>(
		config.auth_scopes ?? ['openid', 'profile', 'email']
	);
	let newScope = $state('');

	// Auth type
	let authType = $state<OAuth2AuthType>(config.auth_type ?? 'url_encoded_body');

	// Token behavior
	let forwardBearerToken = $state(config.forward_bearer_token ?? true);
	let preserveAuthorizationHeader = $state(config.preserve_authorization_header ?? false);
	let useRefreshToken = $state(config.use_refresh_token ?? false);
	let defaultExpiresInSeconds = $state<number | undefined>(config.default_expires_in_seconds);

	// Stats
	let statPrefix = $state(config.stat_prefix ?? '');

	// Pass-through matchers
	let passThrough = $state<PassThroughMatcher[]>(
		config.pass_through_matcher ?? []
	);

	// Advanced
	let showAdvanced = $state(false);
	let showPassThrough = $state(passThrough.length > 0);

	// Validation errors
	let validationErrors = $state<string[]>([]);

	const AUTH_TYPES: { value: OAuth2AuthType; label: string }[] = [
		{ value: 'url_encoded_body', label: 'URL Encoded Body' },
		{ value: 'basic_auth', label: 'Basic Auth' }
	];

	function updateParent() {
		const cfg: OAuth2Config = {
			token_endpoint: {
				uri: tokenUri,
				cluster: tokenCluster,
				timeout_ms: tokenTimeoutMs
			},
			authorization_endpoint: authorizationEndpoint,
			credentials: {
				client_id: clientId,
				token_secret: tokenSecretName ? { name: tokenSecretName } : undefined,
				cookie_domain: cookieDomain || undefined
			},
			redirect_uri: redirectUri,
			redirect_path: redirectPath || undefined,
			signout_path: signoutPath || undefined,
			auth_scopes: authScopes.length > 0 ? authScopes : undefined,
			auth_type: authType,
			forward_bearer_token: forwardBearerToken,
			preserve_authorization_header: preserveAuthorizationHeader || undefined,
			use_refresh_token: useRefreshToken || undefined,
			default_expires_in_seconds: defaultExpiresInSeconds,
			stat_prefix: statPrefix || undefined,
			pass_through_matcher: passThrough.length > 0 ? passThrough : undefined
		};

		const result = OAuth2ConfigSchema.safeParse(cfg);
		validationErrors = result.success
			? []
			: result.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`);

		onConfigChange(cfg);
	}

	function addScope() {
		const trimmed = newScope.trim();
		if (trimmed && !authScopes.includes(trimmed)) {
			authScopes = [...authScopes, trimmed];
			newScope = '';
			updateParent();
		}
	}

	function removeScope(scope: string) {
		authScopes = authScopes.filter((s) => s !== scope);
		updateParent();
	}

	function addPassThrough() {
		passThrough = [...passThrough, { path_prefix: '' }];
		updateParent();
	}

	function removePassThrough(index: number) {
		passThrough = passThrough.filter((_, i) => i !== index);
		updateParent();
	}

	function updatePassThrough(index: number, field: keyof PassThroughMatcher, value: string) {
		const matcher = { ...passThrough[index] };
		// Clear other path fields when setting one
		if (field === 'path_exact' || field === 'path_prefix' || field === 'path_regex') {
			delete matcher.path_exact;
			delete matcher.path_prefix;
			delete matcher.path_regex;
			delete matcher.header_name;
			delete matcher.header_value;
		}
		if (value) {
			matcher[field] = value;
		}
		const newPt = [...passThrough];
		newPt[index] = matcher;
		passThrough = newPt;
		updateParent();
	}

	function updatePassThroughType(index: number, type: string) {
		const matcher: PassThroughMatcher = {};
		if (type === 'path_exact') matcher.path_exact = passThrough[index].path_exact ?? '';
		else if (type === 'path_prefix') matcher.path_prefix = passThrough[index].path_prefix ?? '';
		else if (type === 'path_regex') matcher.path_regex = passThrough[index].path_regex ?? '';
		else if (type === 'header') {
			matcher.header_name = passThrough[index].header_name ?? '';
			matcher.header_value = passThrough[index].header_value ?? '';
		}
		const newPt = [...passThrough];
		newPt[index] = matcher;
		passThrough = newPt;
		updateParent();
	}

	function getMatcherType(matcher: PassThroughMatcher): string {
		if (matcher.path_exact !== undefined) return 'path_exact';
		if (matcher.path_prefix !== undefined) return 'path_prefix';
		if (matcher.path_regex !== undefined) return 'path_regex';
		if (matcher.header_name !== undefined) return 'header';
		return 'path_prefix';
	}

	function getMatcherValue(matcher: PassThroughMatcher): string {
		return matcher.path_exact ?? matcher.path_prefix ?? matcher.path_regex ?? '';
	}

	function handleScopeKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			addScope();
		}
	}
</script>

<div class="space-y-6">
	<!-- Info Box -->
	<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
		<div class="flex items-start gap-2">
			<Info class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0" />
			<div class="text-xs text-blue-700">
				<p class="font-medium">OAuth2 Authentication</p>
				<p class="mt-1">
					Enables OAuth2 authentication flows for HTTP requests. Redirects unauthenticated
					users to the authorization endpoint and handles the OAuth2 callback. Requires a
					cluster for the token endpoint and an SDS secret for the client secret.
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

	<!-- Token Endpoint -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Token Endpoint</h3>
		<div class="space-y-3">
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">URI</label>
				<input
					type="text"
					bind:value={tokenUri}
					oninput={updateParent}
					placeholder="https://auth.example.com/oauth/token"
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
			</div>
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Cluster</label>
				<input
					type="text"
					bind:value={tokenCluster}
					oninput={updateParent}
					placeholder="auth-cluster"
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
				<p class="text-xs text-gray-500 mt-1">
					Envoy cluster name for the token endpoint
				</p>
			</div>
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Timeout (ms)</label>
				<input
					type="number"
					bind:value={tokenTimeoutMs}
					oninput={updateParent}
					min="1"
					class="w-32 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
			</div>
		</div>
	</div>

	<!-- Authorization -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Authorization</h3>
		<div class="space-y-3">
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Authorization Endpoint</label>
				<input
					type="text"
					bind:value={authorizationEndpoint}
					oninput={updateParent}
					placeholder="https://auth.example.com/authorize"
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
			</div>
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Auth Type</label>
				<select
					bind:value={authType}
					onchange={updateParent}
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				>
					{#each AUTH_TYPES as at}
						<option value={at.value}>{at.label}</option>
					{/each}
				</select>
			</div>
		</div>
	</div>

	<!-- Credentials -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Credentials</h3>
		<div class="space-y-3">
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Client ID</label>
				<input
					type="text"
					bind:value={clientId}
					oninput={updateParent}
					placeholder="my-oauth-client-id"
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
			</div>
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Token Secret Name (SDS)</label>
				<input
					type="text"
					bind:value={tokenSecretName}
					oninput={updateParent}
					placeholder="oauth2-token-secret"
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
				<p class="text-xs text-gray-500 mt-1">
					Name of the SDS secret containing the client secret
				</p>
			</div>
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Cookie Domain</label>
				<input
					type="text"
					bind:value={cookieDomain}
					oninput={updateParent}
					placeholder=".example.com (optional)"
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
			</div>
		</div>
	</div>

	<!-- Redirect -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Redirect Settings</h3>
		<div class="space-y-3">
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Redirect URI</label>
				<input
					type="text"
					bind:value={redirectUri}
					oninput={updateParent}
					placeholder="https://app.example.com/oauth2/callback"
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
			</div>
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Callback Path</label>
				<input
					type="text"
					bind:value={redirectPath}
					oninput={updateParent}
					placeholder="/oauth2/callback"
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
			</div>
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Sign-out Path</label>
				<input
					type="text"
					bind:value={signoutPath}
					oninput={updateParent}
					placeholder="/oauth2/signout (optional)"
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
			</div>
		</div>
	</div>

	<!-- Scopes -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">OAuth2 Scopes</h3>
		<div class="flex items-center gap-2 mb-2">
			<input
				type="text"
				bind:value={newScope}
				onkeydown={handleScopeKeydown}
				placeholder="e.g., openid"
				class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
			/>
			<button
				type="button"
				onclick={addScope}
				class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
			>
				<Plus class="w-4 h-4" />
			</button>
		</div>
		{#if authScopes.length > 0}
			<div class="flex flex-wrap gap-1.5">
				{#each authScopes as scope}
					<span class="inline-flex items-center gap-1 px-2 py-1 bg-gray-100 text-gray-700 text-xs rounded-md">
						{scope}
						<button
							type="button"
							onclick={() => removeScope(scope)}
							class="text-gray-400 hover:text-red-500"
						>
							<Trash2 class="w-3 h-3" />
						</button>
					</span>
				{/each}
			</div>
		{/if}
	</div>

	<!-- Token Behavior -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Token Behavior</h3>
		<div class="space-y-3">
			<label class="flex items-center gap-3">
				<input
					type="checkbox"
					bind:checked={forwardBearerToken}
					onchange={updateParent}
					class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
				/>
				<div>
					<span class="text-sm font-medium text-gray-700">Forward Bearer Token</span>
					<p class="text-xs text-gray-500">Forward the OAuth2 bearer token to upstream services</p>
				</div>
			</label>

			<label class="flex items-center gap-3">
				<input
					type="checkbox"
					bind:checked={preserveAuthorizationHeader}
					onchange={updateParent}
					class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
				/>
				<div>
					<span class="text-sm font-medium text-gray-700">Preserve Authorization Header</span>
					<p class="text-xs text-gray-500">Keep existing Authorization header if present</p>
				</div>
			</label>

			<label class="flex items-center gap-3">
				<input
					type="checkbox"
					bind:checked={useRefreshToken}
					onchange={updateParent}
					class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
				/>
				<div>
					<span class="text-sm font-medium text-gray-700">Use Refresh Tokens</span>
					<p class="text-xs text-gray-500">Automatically renew tokens using refresh tokens</p>
				</div>
			</label>
		</div>
	</div>

	<!-- Pass-Through Matchers -->
	<div>
		<button
			type="button"
			onclick={() => (showPassThrough = !showPassThrough)}
			class="flex items-center gap-2 text-sm font-medium text-gray-600 hover:text-gray-900"
		>
			<Settings class="w-4 h-4" />
			<ChevronRight class="w-4 h-4 transition-transform {showPassThrough ? 'rotate-90' : ''}" />
			Pass-Through Matchers (Bypass OAuth2)
		</button>

		{#if showPassThrough}
			<div class="mt-4 space-y-3 pl-6 border-l-2 border-gray-200">
				<p class="text-xs text-gray-500">
					Requests matching these rules bypass OAuth2 authentication entirely.
					This is the only way to make routes public since OAuth2 does not support per-route config.
				</p>

				{#each passThrough as matcher, i}
					<div class="flex items-start gap-2 p-3 bg-gray-50 rounded-md">
						<select
							value={getMatcherType(matcher)}
							onchange={(e) => updatePassThroughType(i, e.currentTarget.value)}
							class="w-32 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
						>
							<option value="path_exact">Exact path</option>
							<option value="path_prefix">Path prefix</option>
							<option value="path_regex">Path regex</option>
							<option value="header">Header</option>
						</select>

						{#if getMatcherType(matcher) === 'header'}
							<input
								type="text"
								value={matcher.header_name ?? ''}
								oninput={(e) => updatePassThrough(i, 'header_name', e.currentTarget.value)}
								placeholder="Header name"
								class="w-32 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
							<input
								type="text"
								value={matcher.header_value ?? ''}
								oninput={(e) => updatePassThrough(i, 'header_value', e.currentTarget.value)}
								placeholder="Header value"
								class="flex-1 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
						{:else}
							<input
								type="text"
								value={getMatcherValue(matcher)}
								oninput={(e) => updatePassThrough(i, getMatcherType(matcher) as keyof PassThroughMatcher, e.currentTarget.value)}
								placeholder={getMatcherType(matcher) === 'path_exact' ? '/healthz' : getMatcherType(matcher) === 'path_prefix' ? '/api/public/' : '^/static/.*'}
								class="flex-1 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
						{/if}

						<button
							type="button"
							onclick={() => removePassThrough(i)}
							class="text-gray-400 hover:text-red-500 p-1"
						>
							<Trash2 class="w-3.5 h-3.5" />
						</button>
					</div>
				{/each}

				<button
					type="button"
					onclick={addPassThrough}
					class="flex items-center gap-1 text-xs text-blue-600 hover:text-blue-800"
				>
					<Plus class="w-3.5 h-3.5" />
					Add pass-through rule
				</button>
			</div>
		{/if}
	</div>

	<!-- Advanced -->
	<div>
		<button
			type="button"
			onclick={() => (showAdvanced = !showAdvanced)}
			class="flex items-center gap-2 text-sm font-medium text-gray-600 hover:text-gray-900"
		>
			<Settings class="w-4 h-4" />
			<ChevronRight class="w-4 h-4 transition-transform {showAdvanced ? 'rotate-90' : ''}" />
			Advanced Settings
		</button>

		{#if showAdvanced}
			<div class="mt-4 space-y-4 pl-6 border-l-2 border-gray-200">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Default Expires In (seconds)
					</label>
					<input
						type="number"
						value={defaultExpiresInSeconds ?? ''}
						oninput={(e) => {
							const val = e.currentTarget.value;
							defaultExpiresInSeconds = val ? parseInt(val) : undefined;
							updateParent();
						}}
						min="0"
						placeholder="3600"
						class="w-32 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Fallback expiration if the token endpoint doesn't provide one
					</p>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Stat Prefix
					</label>
					<input
						type="text"
						bind:value={statPrefix}
						oninput={updateParent}
						placeholder="Optional metrics prefix"
						class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
				</div>
			</div>
		{/if}
	</div>
</div>
