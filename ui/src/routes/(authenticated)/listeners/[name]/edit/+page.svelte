<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { ArrowLeft, ChevronDown, ChevronUp, Link as LinkIcon, ExternalLink } from 'lucide-svelte';
	import { selectedTeam } from '$lib/stores/team';
	import type { ListenerResponse, ListenerFilterChainInput, ListenerFilterInput } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import JsonPanel from '$lib/components/route-config/JsonPanel.svelte';
	import FilterChainList from '$lib/components/listener/FilterChainList.svelte';
	import Badge from '$lib/components/Badge.svelte';

	interface FormState {
		name: string;
		team: string;
		address: string;
		port: number;
		protocol: string;
		filterChains: ListenerFilterChainInput[];
	}

	let currentTeam = $state<string>('');
	let error = $state<string | null>(null);
	let isSubmitting = $state(false);
	let isLoading = $state(true);
	let activeTab = $state<'configuration' | 'json'>('configuration');
	let filterChainsExpanded = $state(true);
	let listenerName = $state<string>('');

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Initialize form state
	let formState = $state<FormState>({
		name: '',
		team: currentTeam,
		address: '0.0.0.0',
		port: 8080,
		protocol: 'HTTP',
		filterChains: []
	});

	// Get listener name from URL params
	$effect(() => {
		const name = $page.params.name;
		if (name) {
			listenerName = decodeURIComponent(name);
			loadListener();
		}
	});

	// Load listener data
	async function loadListener() {
		if (!listenerName) return;

		isLoading = true;
		error = null;

		try {
			const listener = await apiClient.getListener(listenerName);
			formState = parseListenerToForm(listener);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load listener';
		} finally {
			isLoading = false;
		}
	}

	// Parse listener response to form state
	function parseListenerToForm(listener: ListenerResponse): FormState {
		const config = listener.config || {};

		// Parse filter chains
		const chains = (config.filter_chains || []) as any[];
		const filterChains: ListenerFilterChainInput[] = chains.map((fc) => {
			const filters: ListenerFilterInput[] = (fc.filters || []).map((f: any) => {
				if (f.filter_type?.HttpConnectionManager) {
					return {
						name: f.name || 'http_connection_manager',
						type: 'httpConnectionManager' as const,
						routeConfigName: f.filter_type.HttpConnectionManager.route_config_name,
						inlineRouteConfig: f.filter_type.HttpConnectionManager.inline_route_config,
						accessLog: f.filter_type.HttpConnectionManager.access_log,
						tracing: f.filter_type.HttpConnectionManager.tracing,
						httpFilters: f.filter_type.HttpConnectionManager.http_filters
					};
				} else if (f.filter_type?.TcpProxy) {
					return {
						name: f.name || 'tcp_proxy',
						type: 'tcpProxy' as const,
						cluster: f.filter_type.TcpProxy.cluster,
						accessLog: f.filter_type.TcpProxy.access_log
					};
				}
				return {
					name: f.name || 'unknown',
					type: 'httpConnectionManager' as const
				};
			});

			return {
				name: fc.name,
				filters,
				tlsContext: fc.tls_context ? {
					certChainFile: fc.tls_context.cert_chain_file,
					privateKeyFile: fc.tls_context.private_key_file,
					caCertFile: fc.tls_context.ca_cert_file,
					requireClientCertificate: fc.tls_context.require_client_certificate,
					minTlsVersion: fc.tls_context.min_tls_version || 'V1_2'
				} : undefined
			};
		});

		return {
			name: listener.name,
			team: listener.team,
			address: listener.address,
			port: listener.port,
			protocol: listener.protocol || 'HTTP',
			filterChains
		};
	}

	// Build JSON payload from form state
	let jsonPayload = $derived(buildListenerJSON(formState));

	// Extract route config names from filter chains
	let routeConfigNames = $derived(() => {
		const names: string[] = [];
		for (const chain of formState.filterChains) {
			for (const filter of chain.filters) {
				if (filter.type === 'httpConnectionManager' && filter.routeConfigName) {
					names.push(filter.routeConfigName);
				}
			}
		}
		return [...new Set(names)]; // Remove duplicates
	});

	function buildListenerJSON(form: FormState): string {
		// Clean up filter chains by removing empty optional fields
		const cleanedFilterChains = form.filterChains.map(chain => {
			const cleanedFilters = chain.filters.map(filter => {
				const cleanedFilter: any = {
					name: filter.name,
					type: filter.type
				};

				// Only include non-empty optional fields
				if (filter.routeConfigName && filter.routeConfigName.trim() !== '') {
					cleanedFilter.routeConfigName = filter.routeConfigName;
				}
				if (filter.inlineRouteConfig) {
					cleanedFilter.inlineRouteConfig = filter.inlineRouteConfig;
				}
				if (filter.accessLog) {
					cleanedFilter.accessLog = filter.accessLog;
				}
				if (filter.tracing) {
					cleanedFilter.tracing = filter.tracing;
				}
				if (filter.httpFilters) {
					cleanedFilter.httpFilters = filter.httpFilters;
				}
				if (filter.cluster) {
					cleanedFilter.cluster = filter.cluster;
				}

				return cleanedFilter;
			});

			const cleanedChain: any = {
				name: chain.name,
				filters: cleanedFilters
			};

			if (chain.tlsContext) {
				cleanedChain.tlsContext = chain.tlsContext;
			}

			return cleanedChain;
		});

		const payload: any = {
			address: form.address || '0.0.0.0',
			port: form.port || 8080,
			protocol: form.protocol || 'HTTP',
			filterChains: cleanedFilterChains
		};

		return JSON.stringify(payload, null, 2);
	}

	// Validate form
	function validateForm(): string | null {
		if (!formState.address) return 'Address is required';
		if (formState.port < 1 || formState.port > 65535) return 'Port must be between 1 and 65535';

		// Validate filter chains
		if (formState.filterChains.length === 0) {
			return 'At least one filter chain is required';
		}

		// Validate HTTP connection manager filters have route config
		for (const chain of formState.filterChains) {
			for (const filter of chain.filters) {
				if (filter.type === 'httpConnectionManager') {
					const hasRouteConfig = filter.routeConfigName && filter.routeConfigName.trim() !== '';
					const hasInlineConfig = filter.inlineRouteConfig;
					if (!hasRouteConfig && !hasInlineConfig) {
						return 'HTTP connection manager filter requires a route config name or inline route config';
					}
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
			const payload = JSON.parse(jsonPayload);
			console.log('Updating listener:', payload);
			await apiClient.updateListener(formState.name, payload);
			goto('/listeners');
		} catch (e) {
			console.error('Update failed:', e);
			error = e instanceof Error ? e.message : 'Failed to update listener';
		} finally {
			isSubmitting = false;
		}
	}

	// Handle cancel
	function handleCancel() {
		goto('/listeners');
	}

	// Handle filter chains change
	function handleFilterChainsChange(chains: ListenerFilterChainInput[]) {
		formState.filterChains = chains;
	}
</script>

<div class="min-h-screen bg-gray-50">
	<div class="px-8 py-8">
		<!-- Loading State -->
		{#if isLoading}
			<div class="flex items-center justify-center py-12">
				<div class="flex flex-col items-center gap-3">
					<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
					<span class="text-sm text-gray-600">Loading listener...</span>
				</div>
			</div>
		{:else}
			<!-- Tabs -->
			<div class="mb-6">
				<div class="border-b border-gray-200">
					<nav class="-mb-px flex space-x-8" aria-label="Tabs">
						<button
							onclick={() => (activeTab = 'configuration')}
							class="{activeTab === 'configuration'
								? 'border-blue-500 text-blue-600'
								: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'} whitespace-nowrap py-4 px-1 border-b-2 font-medium text-sm"
						>
							Configuration
						</button>
						<button
							onclick={() => (activeTab = 'json')}
							class="{activeTab === 'json'
								? 'border-blue-500 text-blue-600'
								: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'} whitespace-nowrap py-4 px-1 border-b-2 font-medium text-sm"
						>
							JSON Preview
						</button>
					</nav>
				</div>
			</div>

			<!-- Tab Content -->
			{#if activeTab === 'configuration'}
				<!-- Header -->
				<div class="mb-8">
					<div class="flex items-center gap-4 mb-4">
						<button
							onclick={handleCancel}
							class="text-blue-600 hover:text-blue-800 transition-colors"
							title="Back to list"
						>
							<ArrowLeft class="w-6 h-6" />
						</button>
						<div>
							<h1 class="text-3xl font-bold text-gray-900">Edit Listener</h1>
							<p class="text-sm text-gray-600 mt-1">
								Update listener configuration
							</p>
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
								Listener Name
							</label>
							<input
								type="text"
								value={formState.name}
								disabled
								class="w-full px-3 py-2 border border-gray-300 rounded-md bg-gray-100 cursor-not-allowed"
							/>
							<p class="text-xs text-gray-500 mt-1">
								Listener name cannot be changed
							</p>
						</div>

						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">
								Team
							</label>
							<input
								type="text"
								value={formState.team}
								disabled
								class="w-full px-3 py-2 border border-gray-300 rounded-md bg-gray-100 cursor-not-allowed"
							/>
							<p class="text-xs text-gray-500 mt-1">
								Team cannot be changed
							</p>
						</div>
					</div>
				</div>

				<!-- Associated Route Configs -->
				{#if routeConfigNames().length > 0}
					<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
						<h2 class="text-lg font-semibold text-gray-900 mb-4">Associated Route Configs</h2>
						<p class="text-sm text-gray-600 mb-4">
							This listener is using the following route configurations
						</p>
						<div class="space-y-2">
							{#each routeConfigNames() as routeConfigName}
								<div class="flex items-center justify-between p-3 bg-blue-50 border border-blue-200 rounded-md">
									<div class="flex items-center gap-2">
										<LinkIcon class="h-4 w-4 text-blue-600" />
										<span class="font-medium text-gray-900">{routeConfigName}</span>
									</div>
									<button
										onclick={() => goto(`/route-configs/${encodeURIComponent(routeConfigName)}/edit`)}
										class="flex items-center gap-1 text-sm text-blue-600 hover:text-blue-800 transition-colors"
									>
										View
										<ExternalLink class="h-3 w-3" />
									</button>
								</div>
							{/each}
						</div>
					</div>
				{/if}

				<!-- Network Settings -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Network Settings</h2>
					<div class="space-y-4">
						<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
							<div>
								<label class="block text-sm font-medium text-gray-700 mb-1">
									Address <span class="text-red-500">*</span>
								</label>
								<input
									type="text"
									bind:value={formState.address}
									placeholder="0.0.0.0"
									class="w-full px-3 py-2 border border-gray-300 rounded-md font-mono focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<p class="text-xs text-gray-500 mt-1">
									IP address to bind to (0.0.0.0 for all interfaces)
								</p>
							</div>

							<div>
								<label class="block text-sm font-medium text-gray-700 mb-1">
									Port <span class="text-red-500">*</span>
								</label>
								<input
									type="number"
									bind:value={formState.port}
									min="1"
									max="65535"
									placeholder="8080"
									class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<p class="text-xs text-gray-500 mt-1">
									Port number (1-65535)
								</p>
							</div>
						</div>

						<div class="w-48">
							<label class="block text-sm font-medium text-gray-700 mb-1">
								Protocol
							</label>
							<select
								bind:value={formState.protocol}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							>
								<option value="HTTP">HTTP</option>
								<option value="TCP">TCP</option>
							</select>
							<p class="text-xs text-gray-500 mt-1">
								Network protocol
							</p>
						</div>
					</div>
				</div>

				<!-- Filter Chains (Collapsible) -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 mb-6">
					<button
						onclick={() => (filterChainsExpanded = !filterChainsExpanded)}
						class="w-full px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
					>
						<div class="text-left">
							<h2 class="text-lg font-semibold text-gray-900">Filter Chains</h2>
							<p class="text-sm text-gray-600">Configure routing and TLS settings</p>
						</div>
						{#if filterChainsExpanded}
							<ChevronUp class="w-5 h-5 text-gray-500" />
						{:else}
							<ChevronDown class="w-5 h-5 text-gray-500" />
						{/if}
					</button>
					{#if filterChainsExpanded}
						<div class="px-6 pb-6">
							<FilterChainList
								filterChains={formState.filterChains}
								onFilterChainsChange={handleFilterChainsChange}
								compact={false}
							/>
						</div>
					{/if}
				</div>

				<!-- Action Buttons -->
				<div class="sticky bottom-0 bg-white border-t border-gray-200 p-4 -mx-8 flex justify-end gap-3">
					<Button onclick={handleCancel} variant="secondary" disabled={isSubmitting}>
						Cancel
					</Button>
					<Button onclick={handleSubmit} variant="primary" disabled={isSubmitting}>
						{isSubmitting ? 'Updating...' : 'Update Listener'}
					</Button>
				</div>
			{:else}
				<!-- JSON Tab -->
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
					<JsonPanel jsonString={jsonPayload} editable={false} />
				</div>
			{/if}
		{/if}
	</div>
</div>
