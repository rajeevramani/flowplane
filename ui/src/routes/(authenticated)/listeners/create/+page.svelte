<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { ChevronDown, ChevronUp } from 'lucide-svelte';
	import { selectedTeam } from '$lib/stores/team';
	import type { ListenerFilterChainInput, RouteResponse, ListenerResponse } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import JsonPanel from '$lib/components/route-config/JsonPanel.svelte';
	import FilterChainList from '$lib/components/listener/FilterChainList.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import { ErrorAlert, FormActions, PageHeader } from '$lib/components/forms';
	import { validateRequired, validateIdentifier, validatePort, runValidators } from '$lib/utils/validators';

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
	let isLoadingData = $state(true);
	let activeTab = $state<'configuration' | 'json'>('configuration');
	let filterChainsExpanded = $state(true);

	// Route configs and listeners data
	let routeConfigs = $state<RouteResponse[]>([]);
	let listeners = $state<ListenerResponse[]>([]);
	let routeConfigSearch = $state('');
	let routeConfigDropdownOpen = $state(false);
	let selectedRouteConfigName = $state<string>('');

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Load route configs and listeners on mount
	onMount(async () => {
		await loadData();
	});

	async function loadData() {
		isLoadingData = true;
		try {
			const [routesData, listenersData] = await Promise.all([
				apiClient.listRouteConfigs(),
				apiClient.listListeners()
			]);
			routeConfigs = routesData;
			listeners = listenersData;
		} catch (e) {
			console.error('Failed to load data:', e);
		} finally {
			isLoadingData = false;
		}
	}

	// Get listener using a route config
	function getListenerForRouteConfig(routeConfigName: string): ListenerResponse | null {
		for (const listener of listeners) {
			const config = listener.config || {};
			const filterChains = config.filter_chains || [];

			for (const fc of filterChains as any[]) {
				const filters = fc.filters || [];
				for (const filter of filters) {
					if (filter.filter_type?.HttpConnectionManager?.route_config_name === routeConfigName) {
						return listener;
					}
				}
			}
		}
		return null;
	}

	// Get total route count from route config
	function getRouteCount(routeConfig: RouteResponse): number {
		const config = routeConfig.config as any;
		const virtualHosts = config?.virtualHosts || [];
		return virtualHosts.reduce((total: number, vh: any) => {
			return total + (vh.routes?.length || 0);
		}, 0);
	}

	// Filter route configs by search
	let filteredRouteConfigs = $derived(
		routeConfigSearch
			? routeConfigs.filter(rc =>
				rc.name.toLowerCase().includes(routeConfigSearch.toLowerCase())
			)
			: routeConfigs
	);

	// Handle route config selection
	function handleSelectRouteConfig(routeConfigName: string) {
		// Update the first filter chain's route config name
		if (formState.filterChains.length > 0 && formState.filterChains[0].filters.length > 0) {
			const firstFilter = formState.filterChains[0].filters[0];
			if (firstFilter.type === 'httpConnectionManager') {
				firstFilter.routeConfigName = routeConfigName;
				// Trigger reactivity
				formState.filterChains = [...formState.filterChains];
			}
		}
		// Update UI state
		selectedRouteConfigName = routeConfigName;
		routeConfigSearch = routeConfigName;
		routeConfigDropdownOpen = false;
	}

	// Initialize form state with default filter chain
	let formState = $state<FormState>({
		name: '',
		team: currentTeam,
		address: '0.0.0.0',
		port: 8080,
		protocol: 'HTTP',
		filterChains: [
			{
				name: 'default',
				filters: [
					{
						name: 'http_connection_manager',
						type: 'httpConnectionManager',
						routeConfigName: '',
						httpFilters: [
							{
								filter: {
									type: 'router'
								}
							}
						]
					}
				]
			}
		]
	});

	// Build JSON payload from form state
	let jsonPayload = $derived(buildListenerJSON(formState));

	function buildListenerJSON(form: FormState): string {
		// Clean up filter chains by removing empty optional fields
		const cleanedFilterChains = form.filterChains.map(chain => {
			const cleanedFilters = chain.filters.map(filter => {
				const cleanedFilter: Record<string, unknown> = {
					name: filter.name,
					type: filter.type
				};

				// Handle httpConnectionManager type
				if (filter.type === 'httpConnectionManager') {
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
				}

				// Handle tcpProxy type
				if (filter.type === 'tcpProxy') {
					if (filter.cluster) {
						cleanedFilter.cluster = filter.cluster;
					}
					if (filter.accessLog) {
						cleanedFilter.accessLog = filter.accessLog;
					}
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
			team: form.team || currentTeam,
			name: form.name || '',
			address: form.address || '0.0.0.0',
			port: form.port || 8080,
			protocol: form.protocol || 'HTTP',
			filterChains: cleanedFilterChains
		};

		return JSON.stringify(payload, null, 2);
	}

	// Validate form
	function validateForm(): string | null {
		// Basic validation using reusable validators
		const basicError = runValidators([
			() => validateRequired(formState.name, 'Listener name'),
			() => validateIdentifier(formState.name, 'Listener name'),
			() => validateRequired(formState.address, 'Address'),
			() => validatePort(formState.port)
		]);
		if (basicError) return basicError;

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
			console.log('Creating listener:', payload);
			await apiClient.createListener(payload);
			goto('/listeners');
		} catch (e) {
			console.error('Create failed:', e);
			error = e instanceof Error ? e.message : 'Failed to create listener';
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
			<!-- Page Header with Back Button -->
			<PageHeader
				title="Create Listener"
				subtitle="Define a new network listener"
				onBack={handleCancel}
			/>

			<!-- Error Message -->
			<ErrorAlert message={error} />

			<!-- Basic Information -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Basic Information</h2>
				<div class="space-y-4">
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Listener Name <span class="text-red-500">*</span>
						</label>
						<input
							type="text"
							bind:value={formState.name}
							placeholder="e.g., api-listener"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<p class="text-xs text-gray-500 mt-1">
							Unique identifier (lowercase, alphanumeric, dashes only)
						</p>
					</div>
				</div>
			</div>

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

			<!-- Route Config Selection -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Route Configuration</h2>
				<div class="space-y-4">
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Select Route Config <span class="text-red-500">*</span>
						</label>
						<div class="relative">
							<input
								type="text"
								bind:value={routeConfigSearch}
								onfocus={() => (routeConfigDropdownOpen = true)}
								oninput={() => (routeConfigDropdownOpen = true)}
								placeholder="Type to search route configs..."
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
							{#if routeConfigDropdownOpen && (routeConfigSearch || routeConfigs.length > 0)}
								<div class="absolute z-10 w-full mt-1 bg-white border border-gray-300 rounded-md shadow-lg max-h-60 overflow-y-auto">
									{#if isLoadingData}
										<div class="flex items-center justify-center py-8">
											<div class="animate-spin rounded-full h-5 w-5 border-b-2 border-blue-600"></div>
											<span class="ml-2 text-sm text-gray-600">Loading...</span>
										</div>
									{:else if routeConfigs.length === 0}
										<div class="px-4 py-8 text-center">
											<p class="text-sm text-gray-600">No route configs found</p>
											<p class="text-xs text-gray-500 mt-1">Create one first</p>
										</div>
									{:else}
										{#each filteredRouteConfigs.slice(0, 50) as routeConfig}
											{@const associatedListener = getListenerForRouteConfig(routeConfig.name)}
											<button
												onclick={() => handleSelectRouteConfig(routeConfig.name)}
												class="w-full text-left px-4 py-3 hover:bg-blue-50 border-b border-gray-100 last:border-b-0 transition-colors"
											>
												<div class="flex items-center justify-between gap-3">
													<div class="flex-1 min-w-0">
														<div class="flex items-center gap-2">
															<span class="font-medium text-gray-900 truncate">{routeConfig.name}</span>
															{#if associatedListener}
																<Badge variant="yellow">In Use</Badge>
															{:else}
																<Badge variant="green">Available</Badge>
															{/if}
														</div>
														{#if associatedListener}
															<div class="mt-1">
																<span class="text-xs text-gray-500">Listener: {associatedListener.name}</span>
															</div>
														{/if}
													</div>
												</div>
											</button>
										{/each}
										{#if filteredRouteConfigs.length === 0}
											<div class="px-4 py-8 text-center text-sm text-gray-600">
												No matching route configs
											</div>
										{/if}
										{#if filteredRouteConfigs.length > 50}
											<div class="px-4 py-2 text-xs text-gray-500 bg-gray-50 text-center border-t">
												Showing first 50 of {filteredRouteConfigs.length}. Refine search to see more.
											</div>
										{/if}
									{/if}
								</div>
							{/if}
						</div>
						<p class="text-xs text-gray-500 mt-1">
							{#if selectedRouteConfigName}
								Selected: <span class="font-medium text-gray-700">{selectedRouteConfigName}</span>
							{:else}
								Type to search and select a route configuration
							{/if}
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
			<FormActions
				{isSubmitting}
				submitLabel="Create Listener"
				submittingLabel="Creating..."
				onSubmit={handleSubmit}
				onCancel={handleCancel}
			/>
		{:else}
			<!-- JSON Tab -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<JsonPanel jsonString={jsonPayload} editable={false} />
			</div>
		{/if}
	</div>
</div>
