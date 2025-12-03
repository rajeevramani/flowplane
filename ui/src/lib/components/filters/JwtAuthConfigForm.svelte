<script lang="ts">
	import type {
		JwtAuthenticationFilterConfig,
		JwtProviderConfig,
		JwtJwksSourceConfig,
		JwtHeaderConfig,
		JwtClaimToHeaderConfig,
		JwtStringMatcherConfig
	} from '$lib/api/types';
	import { Plus, Trash2, ChevronDown, ChevronUp, Settings, Key, Shield, Database, Send } from 'lucide-svelte';

	interface Props {
		config: JwtAuthenticationFilterConfig;
		onConfigChange: (config: JwtAuthenticationFilterConfig) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Track expanded providers and sections
	let expandedProviders = $state<Set<string>>(new Set());
	let expandedSections = $state<Record<string, Set<string>>>({});

	// Toggle provider expansion
	function toggleProvider(name: string) {
		if (expandedProviders.has(name)) {
			expandedProviders.delete(name);
		} else {
			expandedProviders.add(name);
		}
		expandedProviders = new Set(expandedProviders);
	}

	// Toggle section within a provider
	function toggleSection(providerName: string, section: string) {
		if (!expandedSections[providerName]) {
			expandedSections[providerName] = new Set();
		}
		if (expandedSections[providerName].has(section)) {
			expandedSections[providerName].delete(section);
		} else {
			expandedSections[providerName].add(section);
		}
		expandedSections = { ...expandedSections };
	}

	function isSectionExpanded(providerName: string, section: string): boolean {
		return expandedSections[providerName]?.has(section) ?? false;
	}

	// Create default provider
	function createDefaultProvider(): JwtProviderConfig {
		return {
			issuer: '',
			audiences: [],
			clock_skew_seconds: 60,
			forward: false,
			from_headers: [],
			from_params: [],
			from_cookies: [],
			jwks: {
				type: 'remote',
				http_uri: {
					uri: '',
					cluster: '',
					timeout_ms: 5000
				}
			}
		};
	}

	// Add a new provider
	function addProvider() {
		const baseName = 'provider';
		let counter = 1;
		let newName = baseName;
		while (config.providers[newName]) {
			newName = `${baseName}-${counter}`;
			counter++;
		}

		const newProviders = { ...config.providers, [newName]: createDefaultProvider() };
		onConfigChange({ ...config, providers: newProviders });
		expandedProviders.add(newName);
		expandedProviders = new Set(expandedProviders);
		// Auto-expand basic settings for new provider
		expandedSections[newName] = new Set(['basic', 'jwks']);
		expandedSections = { ...expandedSections };
	}

	// Remove a provider
	function removeProvider(name: string) {
		const newProviders = { ...config.providers };
		delete newProviders[name];
		onConfigChange({ ...config, providers: newProviders });
		expandedProviders.delete(name);
		expandedProviders = new Set(expandedProviders);
	}

	// Rename a provider
	function renameProvider(oldName: string, newName: string) {
		if (newName === oldName) return;
		if (!newName.trim()) return;
		if (config.providers[newName]) return;

		const provider = config.providers[oldName];
		const newProviders = { ...config.providers };
		delete newProviders[oldName];
		newProviders[newName] = provider;

		onConfigChange({ ...config, providers: newProviders });

		if (expandedProviders.has(oldName)) {
			expandedProviders.delete(oldName);
			expandedProviders.add(newName);
			expandedProviders = new Set(expandedProviders);
		}
		if (expandedSections[oldName]) {
			expandedSections[newName] = expandedSections[oldName];
			delete expandedSections[oldName];
			expandedSections = { ...expandedSections };
		}
	}

	// Update provider config
	function updateProvider(name: string, updates: Partial<JwtProviderConfig>) {
		const provider = config.providers[name];
		const newProviders = { ...config.providers, [name]: { ...provider, ...updates } };
		onConfigChange({ ...config, providers: newProviders });
	}

	// Update audiences from comma-separated string
	function updateAudiences(name: string, audiencesStr: string) {
		const audiences = audiencesStr
			.split(',')
			.map((a) => a.trim())
			.filter((a) => a);
		updateProvider(name, { audiences });
	}

	// Update JWKS type
	function updateJwksType(name: string, type: 'remote' | 'local') {
		const provider = config.providers[name];
		let jwks: JwtJwksSourceConfig;

		if (type === 'remote') {
			jwks = {
				type: 'remote',
				http_uri: {
					uri: '',
					cluster: '',
					timeout_ms: 5000
				}
			};
		} else {
			jwks = {
				type: 'local',
				inline_string: ''
			};
		}

		updateProvider(name, { jwks });
	}

	// Update remote JWKS fields
	function updateRemoteJwks(name: string, field: string, value: string | number | boolean | undefined) {
		const provider = config.providers[name];
		if (provider.jwks.type !== 'remote') return;

		const jwks = { ...provider.jwks };
		if (field === 'uri' || field === 'cluster' || field === 'timeout_ms') {
			jwks.http_uri = { ...jwks.http_uri, [field]: value };
		} else if (field === 'cache_duration_seconds') {
			jwks.cache_duration_seconds = value as number | undefined;
		} else if (field === 'fast_listener' || field === 'failed_refetch_duration_seconds') {
			jwks.async_fetch = { ...jwks.async_fetch, [field]: value };
		} else if (field === 'num_retries') {
			jwks.retry_policy = { ...jwks.retry_policy, num_retries: value as number | undefined };
		} else if (field === 'base_interval_ms' || field === 'max_interval_ms') {
			jwks.retry_policy = {
				...jwks.retry_policy,
				retry_backoff: { ...jwks.retry_policy?.retry_backoff, [field]: value }
			};
		}
		updateProvider(name, { jwks });
	}

	// Update local JWKS inline string
	function updateLocalInline(name: string, inline_string: string) {
		const provider = config.providers[name];
		if (provider.jwks.type !== 'local') return;
		updateProvider(name, { jwks: { ...provider.jwks, inline_string } });
	}

	// Update local JWKS filename
	function updateLocalFilename(name: string, filename: string) {
		const provider = config.providers[name];
		if (provider.jwks.type !== 'local') return;
		updateProvider(name, { jwks: { ...provider.jwks, filename: filename || undefined, inline_string: undefined } });
	}

	// Add header extraction
	function addFromHeader(name: string) {
		const provider = config.providers[name];
		const from_headers = [...(provider.from_headers || []), { name: '', value_prefix: '' }];
		updateProvider(name, { from_headers });
	}

	// Update header extraction
	function updateFromHeader(providerName: string, index: number, field: 'name' | 'value_prefix', value: string) {
		const provider = config.providers[providerName];
		const from_headers = [...(provider.from_headers || [])];
		from_headers[index] = { ...from_headers[index], [field]: value };
		updateProvider(providerName, { from_headers });
	}

	// Remove header extraction
	function removeFromHeader(name: string, index: number) {
		const provider = config.providers[name];
		const from_headers = (provider.from_headers || []).filter((_, i) => i !== index);
		updateProvider(name, { from_headers });
	}

	// Update from_params
	function updateFromParams(name: string, paramsStr: string) {
		const from_params = paramsStr
			.split(',')
			.map((p) => p.trim())
			.filter((p) => p);
		updateProvider(name, { from_params });
	}

	// Update from_cookies
	function updateFromCookies(name: string, cookiesStr: string) {
		const from_cookies = cookiesStr
			.split(',')
			.map((c) => c.trim())
			.filter((c) => c);
		updateProvider(name, { from_cookies });
	}

	// Add claim to header mapping
	function addClaimToHeader(name: string) {
		const provider = config.providers[name];
		const claim_to_headers = [...(provider.claim_to_headers || []), { header_name: '', claim_name: '' }];
		updateProvider(name, { claim_to_headers });
	}

	// Update claim to header
	function updateClaimToHeader(providerName: string, index: number, field: 'header_name' | 'claim_name', value: string) {
		const provider = config.providers[providerName];
		const claim_to_headers = [...(provider.claim_to_headers || [])];
		claim_to_headers[index] = { ...claim_to_headers[index], [field]: value };
		updateProvider(providerName, { claim_to_headers });
	}

	// Remove claim to header
	function removeClaimToHeader(name: string, index: number) {
		const provider = config.providers[name];
		const claim_to_headers = (provider.claim_to_headers || []).filter((_, i) => i !== index);
		updateProvider(name, { claim_to_headers });
	}

	// Update subjects matcher
	function updateSubjects(name: string, type: string | null, value: string) {
		if (!type) {
			updateProvider(name, { subjects: undefined });
		} else {
			updateProvider(name, { subjects: { type, value } as JwtStringMatcherConfig });
		}
	}

	// Update normalize payload claims
	function updateNormalizePayloadClaims(name: string, claimsStr: string) {
		const claims = claimsStr
			.split(',')
			.map((c) => c.trim())
			.filter((c) => c);
		if (claims.length === 0) {
			updateProvider(name, { normalize_payload_in_metadata: undefined });
		} else {
			updateProvider(name, { normalize_payload_in_metadata: { space_delimited_claims: claims } });
		}
	}

	// Update bypass CORS preflight
	function updateBypassCors(bypass: boolean) {
		onConfigChange({ ...config, bypass_cors_preflight: bypass });
	}
</script>

<div class="space-y-6">
	<!-- Providers Section -->
	<div>
		<div class="flex items-center justify-between mb-4">
			<h3 class="text-sm font-semibold text-gray-900">JWT Providers</h3>
			<button
				type="button"
				onclick={addProvider}
				class="flex items-center gap-1 px-3 py-1.5 text-sm bg-blue-600 text-white rounded-md hover:bg-blue-700 transition-colors"
			>
				<Plus class="w-4 h-4" />
				Add Provider
			</button>
		</div>

		{#if Object.keys(config.providers).length === 0}
			<div class="text-center py-8 bg-gray-50 rounded-lg border-2 border-dashed border-gray-200">
				<Key class="w-8 h-8 text-gray-400 mx-auto mb-2" />
				<p class="text-gray-500 text-sm">No providers configured</p>
				<p class="text-gray-400 text-xs mt-1">Add a provider to define JWT validation rules</p>
			</div>
		{:else}
			<div class="space-y-4">
				{#each Object.entries(config.providers) as [name, provider]}
					{@const isExpanded = expandedProviders.has(name)}
					<div class="border border-gray-200 rounded-lg overflow-hidden">
						<!-- Provider Header -->
						<div
							class="flex items-center justify-between px-4 py-3 bg-gray-50 cursor-pointer hover:bg-gray-100"
							onclick={() => toggleProvider(name)}
							onkeydown={(e) => e.key === 'Enter' && toggleProvider(name)}
							role="button"
							tabindex="0"
						>
							<div class="flex items-center gap-3">
								{#if isExpanded}
									<ChevronUp class="w-4 h-4 text-gray-500" />
								{:else}
									<ChevronDown class="w-4 h-4 text-gray-500" />
								{/if}
								<Key class="w-4 h-4 text-blue-600" />
								<span class="font-medium text-gray-900">{name}</span>
								{#if provider.issuer}
									<span class="text-xs text-gray-500 truncate max-w-48">({provider.issuer})</span>
								{/if}
							</div>
							<button
								type="button"
								onclick={(e) => {
									e.stopPropagation();
									removeProvider(name);
								}}
								class="p-1 text-gray-400 hover:text-red-600 hover:bg-red-50 rounded"
								title="Remove provider"
							>
								<Trash2 class="w-4 h-4" />
							</button>
						</div>

						<!-- Provider Details (Expandable) -->
						{#if isExpanded}
							<div class="p-4 space-y-4 border-t border-gray-200">
								<!-- Provider Name -->
								<div>
									<label class="block text-sm font-medium text-gray-700 mb-1">
										Provider Name <span class="text-red-500">*</span>
									</label>
									<input
										type="text"
										value={name}
										onchange={(e) => renameProvider(name, e.currentTarget.value)}
										class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									/>
									<p class="text-xs text-gray-500 mt-1">Unique identifier for this provider</p>
								</div>

								<!-- ===================== BASIC SETTINGS SECTION ===================== -->
								<div class="border border-gray-200 rounded-lg overflow-hidden">
									<button
										type="button"
										class="w-full flex items-center justify-between px-4 py-2 bg-gray-50 hover:bg-gray-100"
										onclick={() => toggleSection(name, 'basic')}
									>
										<span class="flex items-center gap-2 text-sm font-medium text-gray-700">
											<Settings class="w-4 h-4" />
											Basic Settings
										</span>
										{#if isSectionExpanded(name, 'basic')}
											<ChevronUp class="w-4 h-4 text-gray-500" />
										{:else}
											<ChevronDown class="w-4 h-4 text-gray-500" />
										{/if}
									</button>
									{#if isSectionExpanded(name, 'basic')}
										<div class="p-4 space-y-4 border-t border-gray-200">
											<!-- Issuer -->
											<div>
												<label class="block text-sm font-medium text-gray-700 mb-1">
													Issuer <span class="text-amber-600 text-xs">(recommended)</span>
												</label>
												<input
													type="text"
													value={provider.issuer || ''}
													oninput={(e) => updateProvider(name, { issuer: e.currentTarget.value || undefined })}
													placeholder="https://auth.example.com"
													class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
												/>
												<p class="text-xs text-gray-500 mt-1">Expected 'iss' claim value in the JWT</p>
											</div>

											<!-- Audiences -->
											<div>
												<label class="block text-sm font-medium text-gray-700 mb-1">Audiences</label>
												<input
													type="text"
													value={provider.audiences?.join(', ') || ''}
													oninput={(e) => updateAudiences(name, e.currentTarget.value)}
													placeholder="api, mobile-app"
													class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
												/>
												<p class="text-xs text-gray-500 mt-1">Comma-separated list of allowed 'aud' claim values</p>
											</div>
										</div>
									{/if}
								</div>

								<!-- ===================== JWKS SOURCE SECTION ===================== -->
								<div class="border border-gray-200 rounded-lg overflow-hidden">
									<button
										type="button"
										class="w-full flex items-center justify-between px-4 py-2 bg-gray-50 hover:bg-gray-100"
										onclick={() => toggleSection(name, 'jwks')}
									>
										<span class="flex items-center gap-2 text-sm font-medium text-gray-700">
											<Key class="w-4 h-4" />
											JWKS Source <span class="text-red-500">*</span>
										</span>
										{#if isSectionExpanded(name, 'jwks')}
											<ChevronUp class="w-4 h-4 text-gray-500" />
										{:else}
											<ChevronDown class="w-4 h-4 text-gray-500" />
										{/if}
									</button>
									{#if isSectionExpanded(name, 'jwks')}
										<div class="p-4 space-y-4 border-t border-gray-200">
											<!-- JWKS Type -->
											<div>
												<label class="block text-sm font-medium text-gray-700 mb-1">Source Type</label>
												<select
													value={provider.jwks.type}
													onchange={(e) => updateJwksType(name, e.currentTarget.value as 'remote' | 'local')}
													class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
												>
													<option value="remote">Remote URL (fetch from HTTP endpoint)</option>
													<option value="local">Local (inline or file)</option>
												</select>
											</div>

											<!-- Remote JWKS Configuration -->
											{#if provider.jwks.type === 'remote'}
												<div class="p-3 bg-blue-50 rounded-md space-y-4">
													<div class="grid grid-cols-2 gap-4">
														<div class="col-span-2">
															<label class="block text-sm font-medium text-gray-700 mb-1">
																JWKS URI <span class="text-red-500">*</span>
															</label>
															<input
																type="text"
																value={provider.jwks.http_uri.uri}
																oninput={(e) => updateRemoteJwks(name, 'uri', e.currentTarget.value)}
																placeholder="https://auth.example.com/.well-known/jwks.json"
																class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
															/>
														</div>
														<div>
															<label class="block text-sm font-medium text-gray-700 mb-1">
																Cluster Name <span class="text-red-500">*</span>
															</label>
															<input
																type="text"
																value={provider.jwks.http_uri.cluster}
																oninput={(e) => updateRemoteJwks(name, 'cluster', e.currentTarget.value)}
																placeholder="jwks-cluster"
																class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
															/>
															<p class="text-xs text-gray-500 mt-1">Envoy cluster for JWKS fetch</p>
														</div>
														<div>
															<label class="block text-sm font-medium text-gray-700 mb-1">Timeout (ms)</label>
															<input
																type="number"
																value={provider.jwks.http_uri.timeout_ms || 5000}
																oninput={(e) => updateRemoteJwks(name, 'timeout_ms', parseInt(e.currentTarget.value) || 5000)}
																class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
															/>
														</div>
													</div>

													<!-- Advanced Remote Options -->
													<details class="mt-3">
														<summary class="text-xs font-medium text-gray-600 cursor-pointer hover:text-gray-900">
															Advanced Options
														</summary>
														<div class="mt-3 space-y-3 pl-2 border-l-2 border-blue-200">
															<div>
																<label class="block text-xs font-medium text-gray-600 mb-1">Cache Duration (seconds)</label>
																<input
																	type="number"
																	value={provider.jwks.cache_duration_seconds || ''}
																	oninput={(e) => updateRemoteJwks(name, 'cache_duration_seconds', parseInt(e.currentTarget.value) || undefined)}
																	placeholder="600"
																	class="w-full px-2 py-1 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
																/>
															</div>
															<div class="flex items-center gap-2">
																<input
																	type="checkbox"
																	id={`fast-listener-${name}`}
																	checked={provider.jwks.async_fetch?.fast_listener || false}
																	onchange={(e) => updateRemoteJwks(name, 'fast_listener', e.currentTarget.checked)}
																	class="h-4 w-4 text-blue-600 rounded border-gray-300"
																/>
																<label for={`fast-listener-${name}`} class="text-xs text-gray-600">
																	Fast Listener (don't wait for initial fetch)
																</label>
															</div>
															<div>
																<label class="block text-xs font-medium text-gray-600 mb-1">Retry Count</label>
																<input
																	type="number"
																	value={provider.jwks.retry_policy?.num_retries || ''}
																	oninput={(e) => updateRemoteJwks(name, 'num_retries', parseInt(e.currentTarget.value) || undefined)}
																	placeholder="3"
																	class="w-full px-2 py-1 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
																/>
															</div>
														</div>
													</details>
												</div>
											{/if}

											<!-- Local JWKS Configuration -->
											{#if provider.jwks.type === 'local'}
												<div class="p-3 bg-green-50 rounded-md space-y-3">
													<div>
														<label class="block text-sm font-medium text-gray-700 mb-1">Inline JWKS JSON</label>
														<textarea
															value={provider.jwks.inline_string || ''}
															oninput={(e) => updateLocalInline(name, e.currentTarget.value)}
															placeholder={'{"keys": [{"kty": "RSA", ...}]}'}
															rows="4"
															class="w-full px-3 py-2 border border-gray-300 rounded-md font-mono text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
														></textarea>
													</div>
													<div class="text-xs text-gray-500 text-center">- or -</div>
													<div>
														<label class="block text-sm font-medium text-gray-700 mb-1">File Path</label>
														<input
															type="text"
															value={provider.jwks.filename || ''}
															oninput={(e) => updateLocalFilename(name, e.currentTarget.value)}
															placeholder="/etc/envoy/jwks.json"
															class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
														/>
													</div>
												</div>
											{/if}
										</div>
									{/if}
								</div>

								<!-- ===================== TOKEN EXTRACTION SECTION ===================== -->
								<div class="border border-gray-200 rounded-lg overflow-hidden">
									<button
										type="button"
										class="w-full flex items-center justify-between px-4 py-2 bg-gray-50 hover:bg-gray-100"
										onclick={() => toggleSection(name, 'extraction')}
									>
										<span class="flex items-center gap-2 text-sm font-medium text-gray-700">
											<Send class="w-4 h-4" />
											Token Extraction
											<span class="text-xs text-gray-500">(optional - has defaults)</span>
										</span>
										{#if isSectionExpanded(name, 'extraction')}
											<ChevronUp class="w-4 h-4 text-gray-500" />
										{:else}
											<ChevronDown class="w-4 h-4 text-gray-500" />
										{/if}
									</button>
									{#if isSectionExpanded(name, 'extraction')}
										<div class="p-4 space-y-4 border-t border-gray-200">
											<p class="text-xs text-gray-500 mb-3">
												By default, JWT is extracted from Authorization header (Bearer) and access_token query param.
											</p>

											<!-- From Headers -->
											<div>
												<div class="flex items-center justify-between mb-2">
													<label class="text-sm font-medium text-gray-700">From Headers</label>
													<button
														type="button"
														onclick={() => addFromHeader(name)}
														class="text-xs text-blue-600 hover:text-blue-700"
													>
														+ Add Header
													</button>
												</div>
												{#if (provider.from_headers?.length || 0) > 0}
													<div class="space-y-2">
														{#each provider.from_headers || [] as header, i}
															<div class="flex gap-2 items-center">
																<input
																	type="text"
																	value={header.name}
																	oninput={(e) => updateFromHeader(name, i, 'name', e.currentTarget.value)}
																	placeholder="Header name"
																	class="flex-1 px-2 py-1 text-sm border border-gray-300 rounded-md"
																/>
																<input
																	type="text"
																	value={header.value_prefix || ''}
																	oninput={(e) => updateFromHeader(name, i, 'value_prefix', e.currentTarget.value)}
																	placeholder="Prefix (e.g., Bearer )"
																	class="flex-1 px-2 py-1 text-sm border border-gray-300 rounded-md"
																/>
																<button
																	type="button"
																	onclick={() => removeFromHeader(name, i)}
																	class="p-1 text-gray-400 hover:text-red-600"
																>
																	<Trash2 class="w-4 h-4" />
																</button>
															</div>
														{/each}
													</div>
												{:else}
													<p class="text-xs text-gray-400 italic">Using default: Authorization header with Bearer prefix</p>
												{/if}
											</div>

											<!-- From Query Params -->
											<div>
												<label class="block text-sm font-medium text-gray-700 mb-1">From Query Parameters</label>
												<input
													type="text"
													value={provider.from_params?.join(', ') || ''}
													oninput={(e) => updateFromParams(name, e.currentTarget.value)}
													placeholder="token, jwt"
													class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
												/>
												<p class="text-xs text-gray-500 mt-1">Comma-separated. Default: access_token</p>
											</div>

											<!-- From Cookies -->
											<div>
												<label class="block text-sm font-medium text-gray-700 mb-1">From Cookies</label>
												<input
													type="text"
													value={provider.from_cookies?.join(', ') || ''}
													oninput={(e) => updateFromCookies(name, e.currentTarget.value)}
													placeholder="auth-token, session"
													class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
												/>
												<p class="text-xs text-gray-500 mt-1">Comma-separated cookie names</p>
											</div>
										</div>
									{/if}
								</div>

								<!-- ===================== VALIDATION SECTION ===================== -->
								<div class="border border-gray-200 rounded-lg overflow-hidden">
									<button
										type="button"
										class="w-full flex items-center justify-between px-4 py-2 bg-gray-50 hover:bg-gray-100"
										onclick={() => toggleSection(name, 'validation')}
									>
										<span class="flex items-center gap-2 text-sm font-medium text-gray-700">
											<Shield class="w-4 h-4" />
											Validation Settings
										</span>
										{#if isSectionExpanded(name, 'validation')}
											<ChevronUp class="w-4 h-4 text-gray-500" />
										{:else}
											<ChevronDown class="w-4 h-4 text-gray-500" />
										{/if}
									</button>
									{#if isSectionExpanded(name, 'validation')}
										<div class="p-4 space-y-4 border-t border-gray-200">
											<!-- Subject Matcher -->
											<div>
												<label class="block text-sm font-medium text-gray-700 mb-1">Subject Restriction</label>
												<div class="flex gap-2">
													<select
														value={provider.subjects?.type || ''}
														onchange={(e) => {
															const type = e.currentTarget.value || null;
															updateSubjects(name, type, provider.subjects?.value || '');
														}}
														class="w-32 px-2 py-2 border border-gray-300 rounded-md text-sm"
													>
														<option value="">None</option>
														<option value="exact">Exact</option>
														<option value="prefix">Prefix</option>
														<option value="suffix">Suffix</option>
														<option value="contains">Contains</option>
														<option value="regex">Regex</option>
													</select>
													{#if provider.subjects}
														<input
															type="text"
															value={provider.subjects.value}
															oninput={(e) => updateSubjects(name, provider.subjects?.type || 'exact', e.currentTarget.value)}
															placeholder={provider.subjects.type === 'prefix' ? 'spiffe://example.com/' : 'subject pattern'}
															class="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
														/>
													{/if}
												</div>
												<p class="text-xs text-gray-500 mt-1">Restrict allowed 'sub' claim values (e.g., SPIFFE IDs)</p>
											</div>

											<div class="grid grid-cols-2 gap-4">
												<!-- Clock Skew -->
												<div>
													<label class="block text-sm font-medium text-gray-700 mb-1">Clock Skew (seconds)</label>
													<input
														type="number"
														value={provider.clock_skew_seconds || 60}
														oninput={(e) => updateProvider(name, { clock_skew_seconds: parseInt(e.currentTarget.value) || 60 })}
														class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
													/>
													<p class="text-xs text-gray-500 mt-1">Tolerance for exp/nbf validation</p>
												</div>

												<!-- Max Lifetime -->
												<div>
													<label class="block text-sm font-medium text-gray-700 mb-1">Max Lifetime (seconds)</label>
													<input
														type="number"
														value={provider.max_lifetime_seconds || ''}
														oninput={(e) => updateProvider(name, { max_lifetime_seconds: parseInt(e.currentTarget.value) || undefined })}
														placeholder="No limit"
														class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
													/>
													<p class="text-xs text-gray-500 mt-1">Reject tokens with longer lifetime</p>
												</div>
											</div>

											<!-- Require Expiration -->
											<div class="flex items-center gap-2">
												<input
													type="checkbox"
													id={`require-exp-${name}`}
													checked={provider.require_expiration || false}
													onchange={(e) => updateProvider(name, { require_expiration: e.currentTarget.checked })}
													class="h-4 w-4 text-blue-600 rounded border-gray-300"
												/>
												<label for={`require-exp-${name}`} class="text-sm text-gray-700">
													Require expiration claim (exp)
												</label>
											</div>
										</div>
									{/if}
								</div>

								<!-- ===================== FORWARDING & METADATA SECTION ===================== -->
								<div class="border border-gray-200 rounded-lg overflow-hidden">
									<button
										type="button"
										class="w-full flex items-center justify-between px-4 py-2 bg-gray-50 hover:bg-gray-100"
										onclick={() => toggleSection(name, 'forwarding')}
									>
										<span class="flex items-center gap-2 text-sm font-medium text-gray-700">
											<Database class="w-4 h-4" />
											Forwarding & Metadata
										</span>
										{#if isSectionExpanded(name, 'forwarding')}
											<ChevronUp class="w-4 h-4 text-gray-500" />
										{:else}
											<ChevronDown class="w-4 h-4 text-gray-500" />
										{/if}
									</button>
									{#if isSectionExpanded(name, 'forwarding')}
										<div class="p-4 space-y-4 border-t border-gray-200">
											<!-- Forward JWT -->
											<div class="flex items-center gap-2">
												<input
													type="checkbox"
													id={`forward-${name}`}
													checked={provider.forward || false}
													onchange={(e) => updateProvider(name, { forward: e.currentTarget.checked })}
													class="h-4 w-4 text-blue-600 rounded border-gray-300"
												/>
												<label for={`forward-${name}`} class="text-sm text-gray-700">
													Forward original JWT to upstream
												</label>
											</div>

											<!-- Forward Payload Header -->
											<div>
												<label class="block text-sm font-medium text-gray-700 mb-1">Forward Payload Header</label>
												<input
													type="text"
													value={provider.forward_payload_header || ''}
													oninput={(e) => updateProvider(name, { forward_payload_header: e.currentTarget.value || undefined })}
													placeholder="x-jwt-payload"
													class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
												/>
												<p class="text-xs text-gray-500 mt-1">Header to add with base64-encoded JWT payload</p>
											</div>

											<!-- Payload in Metadata -->
											<div>
												<label class="block text-sm font-medium text-gray-700 mb-1">Payload in Metadata Key</label>
												<input
													type="text"
													value={provider.payload_in_metadata || ''}
													oninput={(e) => updateProvider(name, { payload_in_metadata: e.currentTarget.value || undefined })}
													placeholder="jwt_payload"
													class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
												/>
												<p class="text-xs text-gray-500 mt-1">Write JWT payload to dynamic metadata under this key</p>
											</div>

											<!-- Claim to Headers -->
											<div>
												<div class="flex items-center justify-between mb-2">
													<label class="text-sm font-medium text-gray-700">Claim to Headers</label>
													<button
														type="button"
														onclick={() => addClaimToHeader(name)}
														class="text-xs text-blue-600 hover:text-blue-700"
													>
														+ Add Mapping
													</button>
												</div>
												{#if (provider.claim_to_headers?.length || 0) > 0}
													<div class="space-y-2">
														{#each provider.claim_to_headers || [] as mapping, i}
															<div class="flex gap-2 items-center">
																<input
																	type="text"
																	value={mapping.claim_name}
																	oninput={(e) => updateClaimToHeader(name, i, 'claim_name', e.currentTarget.value)}
																	placeholder="Claim (e.g., sub)"
																	class="flex-1 px-2 py-1 text-sm border border-gray-300 rounded-md"
																/>
																<span class="text-gray-400">→</span>
																<input
																	type="text"
																	value={mapping.header_name}
																	oninput={(e) => updateClaimToHeader(name, i, 'header_name', e.currentTarget.value)}
																	placeholder="Header (e.g., x-user-id)"
																	class="flex-1 px-2 py-1 text-sm border border-gray-300 rounded-md"
																/>
																<button
																	type="button"
																	onclick={() => removeClaimToHeader(name, i)}
																	class="p-1 text-gray-400 hover:text-red-600"
																>
																	<Trash2 class="w-4 h-4" />
																</button>
															</div>
														{/each}
													</div>
												{:else}
													<p class="text-xs text-gray-400 italic">No claim-to-header mappings configured</p>
												{/if}
											</div>

											<!-- Clear Route Cache -->
											<div class="flex items-center gap-2">
												<input
													type="checkbox"
													id={`clear-cache-${name}`}
													checked={provider.clear_route_cache || false}
													onchange={(e) => updateProvider(name, { clear_route_cache: e.currentTarget.checked })}
													class="h-4 w-4 text-blue-600 rounded border-gray-300"
												/>
												<label for={`clear-cache-${name}`} class="text-sm text-gray-700">
													Clear route cache when metadata updated
												</label>
											</div>
										</div>
									{/if}
								</div>

								<!-- ===================== CACHE SETTINGS SECTION ===================== -->
								<div class="border border-gray-200 rounded-lg overflow-hidden">
									<button
										type="button"
										class="w-full flex items-center justify-between px-4 py-2 bg-gray-50 hover:bg-gray-100"
										onclick={() => toggleSection(name, 'cache')}
									>
										<span class="flex items-center gap-2 text-sm font-medium text-gray-700">
											<Database class="w-4 h-4" />
											Cache Settings
										</span>
										{#if isSectionExpanded(name, 'cache')}
											<ChevronUp class="w-4 h-4 text-gray-500" />
										{:else}
											<ChevronDown class="w-4 h-4 text-gray-500" />
										{/if}
									</button>
									{#if isSectionExpanded(name, 'cache')}
										<div class="p-4 space-y-4 border-t border-gray-200">
											<div class="grid grid-cols-2 gap-4">
												<div>
													<label class="block text-sm font-medium text-gray-700 mb-1">JWT Cache Size</label>
													<input
														type="number"
														value={provider.jwt_cache_config?.jwt_cache_size || ''}
														oninput={(e) => {
															const size = parseInt(e.currentTarget.value) || undefined;
															updateProvider(name, {
																jwt_cache_config: size || provider.jwt_cache_config?.jwt_max_token_size
																	? { ...provider.jwt_cache_config, jwt_cache_size: size }
																	: undefined
															});
														}}
														placeholder="100"
														class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
													/>
													<p class="text-xs text-gray-500 mt-1">Number of JWTs to cache</p>
												</div>
												<div>
													<label class="block text-sm font-medium text-gray-700 mb-1">Max Token Size (bytes)</label>
													<input
														type="number"
														value={provider.jwt_cache_config?.jwt_max_token_size || ''}
														oninput={(e) => {
															const size = parseInt(e.currentTarget.value) || undefined;
															updateProvider(name, {
																jwt_cache_config: size || provider.jwt_cache_config?.jwt_cache_size
																	? { ...provider.jwt_cache_config, jwt_max_token_size: size }
																	: undefined
															});
														}}
														placeholder="4096"
														class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
													/>
													<p class="text-xs text-gray-500 mt-1">Max size of cached tokens</p>
												</div>
											</div>
										</div>
									{/if}
								</div>
							</div>
						{/if}
					</div>
				{/each}
			</div>
		{/if}
	</div>

	<!-- Global Options -->
	<div class="border-t border-gray-200 pt-6">
		<h3 class="text-sm font-semibold text-gray-900 mb-4">Global Options</h3>
		<div class="flex items-center gap-2">
			<input
				type="checkbox"
				id="bypass-cors"
				checked={config.bypass_cors_preflight || false}
				onchange={(e) => updateBypassCors(e.currentTarget.checked)}
				class="h-4 w-4 text-blue-600 rounded border-gray-300 focus:ring-blue-500"
			/>
			<label for="bypass-cors" class="text-sm text-gray-700">
				Bypass JWT validation for CORS preflight requests
			</label>
		</div>
		<p class="text-xs text-gray-500 mt-1 ml-6">
			When enabled, OPTIONS requests with CORS headers will skip JWT validation
		</p>
	</div>
</div>
