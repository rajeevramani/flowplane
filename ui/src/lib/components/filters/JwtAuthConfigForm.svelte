<script lang="ts">
	import type {
		JwtAuthenticationFilterConfig,
		JwtProviderConfig,
		JwtJwksSourceConfig
	} from '$lib/api/types';
	import { Plus, Trash2, ChevronDown, ChevronUp } from 'lucide-svelte';

	interface Props {
		config: JwtAuthenticationFilterConfig;
		onConfigChange: (config: JwtAuthenticationFilterConfig) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Track expanded providers for accordion UI
	let expandedProviders = $state<Set<string>>(new Set());

	// Toggle provider expansion
	function toggleProvider(name: string) {
		if (expandedProviders.has(name)) {
			expandedProviders.delete(name);
		} else {
			expandedProviders.add(name);
		}
		expandedProviders = new Set(expandedProviders);
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

		const newProvider: JwtProviderConfig = {
			issuer: '',
			audiences: [],
			jwks: {
				type: 'remote',
				http_uri: {
					uri: '',
					cluster: '',
					timeout_seconds: 5
				}
			}
		};

		const newProviders = { ...config.providers, [newName]: newProvider };
		onConfigChange({ ...config, providers: newProviders });
		expandedProviders.add(newName);
		expandedProviders = new Set(expandedProviders);
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
		if (config.providers[newName]) return; // Name already exists

		const provider = config.providers[oldName];
		const newProviders = { ...config.providers };
		delete newProviders[oldName];
		newProviders[newName] = provider;

		onConfigChange({ ...config, providers: newProviders });

		// Update expanded state
		if (expandedProviders.has(oldName)) {
			expandedProviders.delete(oldName);
			expandedProviders.add(newName);
			expandedProviders = new Set(expandedProviders);
		}
	}

	// Update provider config
	function updateProvider(name: string, provider: JwtProviderConfig) {
		const newProviders = { ...config.providers, [name]: provider };
		onConfigChange({ ...config, providers: newProviders });
	}

	// Update issuer
	function updateIssuer(name: string, issuer: string) {
		const provider = config.providers[name];
		updateProvider(name, { ...provider, issuer: issuer || undefined });
	}

	// Update audiences
	function updateAudiences(name: string, audiencesStr: string) {
		const provider = config.providers[name];
		const audiences = audiencesStr
			.split(',')
			.map((a) => a.trim())
			.filter((a) => a);
		updateProvider(name, { ...provider, audiences });
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
					timeout_seconds: 5
				}
			};
		} else {
			jwks = {
				type: 'local',
				inline_string: ''
			};
		}

		updateProvider(name, { ...provider, jwks });
	}

	// Update remote JWKS URI
	function updateRemoteUri(name: string, uri: string) {
		const provider = config.providers[name];
		if (provider.jwks.type !== 'remote') return;

		const jwks: JwtJwksSourceConfig = {
			...provider.jwks,
			http_uri: {
				...provider.jwks.http_uri,
				uri
			}
		};
		updateProvider(name, { ...provider, jwks });
	}

	// Update remote JWKS cluster
	function updateRemoteCluster(name: string, cluster: string) {
		const provider = config.providers[name];
		if (provider.jwks.type !== 'remote') return;

		const jwks: JwtJwksSourceConfig = {
			...provider.jwks,
			http_uri: {
				...provider.jwks.http_uri,
				cluster
			}
		};
		updateProvider(name, { ...provider, jwks });
	}

	// Update local JWKS inline string
	function updateLocalInline(name: string, inline_string: string) {
		const provider = config.providers[name];
		if (provider.jwks.type !== 'local') return;

		const jwks: JwtJwksSourceConfig = {
			...provider.jwks,
			inline_string
		};
		updateProvider(name, { ...provider, jwks });
	}

	// Update forward setting
	function updateForward(name: string, forward: boolean) {
		const provider = config.providers[name];
		updateProvider(name, { ...provider, forward });
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
								<span class="font-medium text-gray-900">{name}</span>
								{#if provider.issuer}
									<span class="text-xs text-gray-500">({provider.issuer})</span>
								{/if}
							</div>
							<button
								type="button"
								onclick={(e) => {
									e.stopPropagation();
									removeProvider(name);
								}}
								class="p-1 text-gray-400 hover:text-red-600 hover:bg-red-50 rounded"
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
										Provider Name
									</label>
									<input
										type="text"
										value={name}
										onchange={(e) => renameProvider(name, e.currentTarget.value)}
										class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									/>
									<p class="text-xs text-gray-500 mt-1">
										Unique identifier for this provider
									</p>
								</div>

								<!-- Issuer -->
								<div>
									<label class="block text-sm font-medium text-gray-700 mb-1">Issuer</label>
									<input
										type="text"
										value={provider.issuer || ''}
										oninput={(e) => updateIssuer(name, e.currentTarget.value)}
										placeholder="https://issuer.example.com"
										class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									/>
									<p class="text-xs text-gray-500 mt-1">
										Expected issuer claim (iss) in the JWT
									</p>
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
									<p class="text-xs text-gray-500 mt-1">
										Comma-separated list of allowed audiences (aud claim)
									</p>
								</div>

								<!-- JWKS Source -->
								<div>
									<label class="block text-sm font-medium text-gray-700 mb-1">JWKS Source</label>
									<select
										value={provider.jwks.type}
										onchange={(e) =>
											updateJwksType(name, e.currentTarget.value as 'remote' | 'local')}
										class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									>
										<option value="remote">Remote URL</option>
										<option value="local">Local/Inline</option>
									</select>
								</div>

								<!-- Remote JWKS Configuration -->
								{#if provider.jwks.type === 'remote'}
									<div class="ml-4 p-3 bg-gray-50 rounded-md space-y-3">
										<div>
											<label class="block text-sm font-medium text-gray-700 mb-1">
												JWKS URI
											</label>
											<input
												type="text"
												value={provider.jwks.http_uri.uri}
												oninput={(e) => updateRemoteUri(name, e.currentTarget.value)}
												placeholder="https://auth.example.com/.well-known/jwks.json"
												class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
											/>
										</div>
										<div>
											<label class="block text-sm font-medium text-gray-700 mb-1">
												Cluster Name
											</label>
											<input
												type="text"
												value={provider.jwks.http_uri.cluster}
												oninput={(e) => updateRemoteCluster(name, e.currentTarget.value)}
												placeholder="jwks-cluster"
												class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
											/>
											<p class="text-xs text-gray-500 mt-1">
												Envoy cluster to use for fetching JWKS
											</p>
										</div>
									</div>
								{/if}

								<!-- Local JWKS Configuration -->
								{#if provider.jwks.type === 'local'}
									<div class="ml-4 p-3 bg-gray-50 rounded-md">
										<label class="block text-sm font-medium text-gray-700 mb-1">
											Inline JWKS
										</label>
										<textarea
											value={provider.jwks.inline_string || ''}
											oninput={(e) => updateLocalInline(name, e.currentTarget.value)}
											placeholder={'{"keys": [...]}'}
											rows="4"
											class="w-full px-3 py-2 border border-gray-300 rounded-md font-mono text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
										></textarea>
										<p class="text-xs text-gray-500 mt-1">
											JSON Web Key Set in JSON format
										</p>
									</div>
								{/if}

								<!-- Forward JWT -->
								<div class="flex items-center gap-2">
									<input
										type="checkbox"
										id={`forward-${name}`}
										checked={provider.forward || false}
										onchange={(e) => updateForward(name, e.currentTarget.checked)}
										class="h-4 w-4 text-blue-600 rounded border-gray-300 focus:ring-blue-500"
									/>
									<label for={`forward-${name}`} class="text-sm text-gray-700">
										Forward JWT to upstream
									</label>
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
