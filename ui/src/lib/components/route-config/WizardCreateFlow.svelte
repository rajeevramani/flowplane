<script lang="ts">
	import { ChevronLeft, Check } from 'lucide-svelte';
	import type { ClusterResponse, CreateRouteBody, RouteResponse } from '$lib/api/types';
	import type { VirtualHostFormState, RouteFormState } from './VirtualHostEditor.svelte';

	interface RouteConfigSummary {
		name: string;
		id: string;
	}

	interface Props {
		availableClusters: ClusterResponse[];
		routeConfigs: RouteConfigSummary[];
		onComplete: (formData: CreateRouteBody) => void;
		onCancel: () => void;
	}

	let { availableClusters, routeConfigs, onComplete, onCancel }: Props = $props();

	// Wizard state
	let currentStep = $state(1);
	let isSubmitting = $state(false);

	// Step 1: Route Config Selection
	let configMode = $state<'existing' | 'new'>('existing');
	let selectedConfigName = $state(routeConfigs.length > 0 ? routeConfigs[0].name : '');
	let newConfigName = $state('');
	let newConfigDescription = $state('');

	// Step 2: Virtual Host Selection
	let vhMode = $state<'existing' | 'new'>('existing');
	let selectedVhName = $state('');
	let newVhName = $state('');
	let newVhDomains = $state('');

	// Step 3: Route Details
	let routeName = $state('');
	let routeMethod = $state('GET');
	let routePath = $state('/');
	let routePathType = $state<'prefix' | 'exact' | 'template' | 'regex'>('prefix');
	let routeCluster = $state(availableClusters.length > 0 ? availableClusters[0].name : '');
	let routeTimeout = $state(30);

	// Advanced settings collapsed by default
	let pathRewriteExpanded = $state(false);
	let timeoutsExpanded = $state(false);
	let mcpExpanded = $state(false);

	// Path rewrite
	let prefixRewrite = $state('');
	let templateRewrite = $state('');

	// Retry policy
	let retryEnabled = $state(false);
	let maxRetries = $state(3);
	let perTryTimeout = $state(10);
	let retryOn = $state<string[]>(['5xx', 'reset', 'connect-failure']);
	let backoffBaseMs = $state(100);
	let backoffMaxMs = $state(1000);

	// MCP configuration
	let mcpEnabled = $state(false);
	let mcpToolName = $state('');
	let mcpDescription = $state('');

	// Get final config name
	let finalConfigName = $derived(
		configMode === 'existing' ? selectedConfigName : newConfigName
	);

	// Validation
	function canProceedFromStep1(): boolean {
		if (configMode === 'existing') {
			return selectedConfigName.length > 0;
		} else {
			return newConfigName.length > 0 && /^[a-z0-9-]+$/.test(newConfigName);
		}
	}

	function canProceedFromStep2(): boolean {
		if (vhMode === 'existing') {
			return selectedVhName.length > 0;
		} else {
			return (
				newVhName.length > 0 &&
				/^[a-z0-9-]+$/.test(newVhName) &&
				newVhDomains.trim().length > 0
			);
		}
	}

	function canSubmit(): boolean {
		return (
			routeName.length > 0 &&
			routePath.length > 0 &&
			routeCluster.length > 0 &&
			/^[a-z0-9-]+$/.test(routeName)
		);
	}

	// Navigation
	function goToStep(step: number) {
		currentStep = step;
	}

	function nextStep() {
		if (currentStep === 1 && canProceedFromStep1()) {
			currentStep = 2;
		} else if (currentStep === 2 && canProceedFromStep2()) {
			currentStep = 3;
		}
	}

	function previousStep() {
		if (currentStep > 1) {
			currentStep--;
		}
	}

	// Submit
	async function handleSubmit() {
		if (!canSubmit()) return;

		isSubmitting = true;

		// Build form data
		const route: RouteFormState = {
			id: `route-${Date.now()}`,
			name: routeName,
			method: routeMethod,
			path: routePath,
			pathType: routePathType,
			cluster: routeCluster,
			timeout: routeTimeout
		};

		// Add path rewrites
		if (routePathType === 'template' && templateRewrite) {
			route.templateRewrite = templateRewrite;
		} else if (prefixRewrite) {
			route.prefixRewrite = prefixRewrite;
		}

		// Add retry policy
		if (retryEnabled) {
			route.retryEnabled = true;
			route.maxRetries = maxRetries;
			route.retryOn = retryOn;
			route.perTryTimeout = perTryTimeout;
			route.backoffBaseMs = backoffBaseMs;
			route.backoffMaxMs = backoffMaxMs;
		}

		// Build virtual host
		const vhName = vhMode === 'existing' ? selectedVhName : newVhName;
		const domains =
			vhMode === 'existing'
				? [] // Existing VH - domains already exist
				: newVhDomains.split(',').map((d) => d.trim()).filter((d) => d.length > 0);

		const virtualHost: VirtualHostFormState = {
			id: `vh-${Date.now()}`,
			name: vhName,
			domains: domains,
			routes: [route]
		};

		// Build create route body
		const formData: CreateRouteBody = {
			team: '', // Will be set by the parent component
			name: finalConfigName,
			virtualHosts: [
				{
					name: virtualHost.name,
					domains: virtualHost.domains,
					routes: virtualHost.routes.map((r) => {
						const action: {
							type: 'forward';
							cluster: string;
							timeoutSeconds: number;
							prefixRewrite?: string;
							templateRewrite?: string;
							retryPolicy?: unknown;
						} = {
							type: 'forward' as const,
							cluster: r.cluster,
							timeoutSeconds: r.timeout || 30
						};

						// Add path rewrites
						if (r.prefixRewrite) {
							action.prefixRewrite = r.prefixRewrite;
						}
						if (r.templateRewrite) {
							action.templateRewrite = r.templateRewrite;
						}

						// Add retry policy
						if (r.retryEnabled) {
							action.retryPolicy = {
								maxRetries: r.maxRetries || 3,
								retryOn: r.retryOn || ['5xx', 'reset', 'connect-failure'],
								perTryTimeoutSeconds: r.perTryTimeout || 10,
								backoff: {
									baseIntervalMs: r.backoffBaseMs || 100,
									maxIntervalMs: r.backoffMaxMs || 1000
								}
							};
						}

						return {
							name: r.name,
							match: {
								path:
									r.pathType === 'template'
										? { type: r.pathType, template: r.path }
										: { type: r.pathType, value: r.path },
								headers: [
									{
										name: ':method',
										value: r.method
									}
								]
							},
							action
						};
					})
				}
			]
		};

		onComplete(formData);
	}
</script>

<div class="space-y-6">
	<!-- Progress Indicator -->
	<div class="flex items-center justify-center mb-8">
		<div class="flex items-center">
			<!-- Step 1 -->
			<div class="flex items-center">
				<button
					onclick={() => currentStep > 1 && goToStep(1)}
					class="w-10 h-10 rounded-full flex items-center justify-center font-semibold transition-colors {currentStep === 1
						? 'bg-blue-600 text-white'
						: currentStep > 1
							? 'bg-green-500 text-white hover:bg-green-600'
							: 'bg-gray-300 text-gray-600'}"
					disabled={currentStep === 1}
				>
					{#if currentStep > 1}
						<Check class="w-5 h-5" />
					{:else}
						1
					{/if}
				</button>
				<div class="ml-2 mr-8">
					<div
						class="text-sm font-medium {currentStep === 1
							? 'text-blue-600'
							: currentStep > 1
								? 'text-green-600'
								: 'text-gray-500'}"
					>
						Route Config
					</div>
					<div class="text-xs text-gray-500">Select or create</div>
				</div>
			</div>
			<div class="w-16 h-0.5 {currentStep > 1 ? 'bg-green-500' : 'bg-gray-300'} mr-8"></div>

			<!-- Step 2 -->
			<div class="flex items-center">
				<button
					onclick={() => currentStep > 2 && goToStep(2)}
					class="w-10 h-10 rounded-full flex items-center justify-center font-semibold transition-colors {currentStep === 2
						? 'bg-blue-600 text-white'
						: currentStep > 2
							? 'bg-green-500 text-white hover:bg-green-600'
							: 'bg-gray-300 text-gray-600'}"
					disabled={currentStep <= 1}
				>
					{#if currentStep > 2}
						<Check class="w-5 h-5" />
					{:else}
						2
					{/if}
				</button>
				<div class="ml-2 mr-8">
					<div
						class="text-sm font-medium {currentStep === 2
							? 'text-blue-600'
							: currentStep > 2
								? 'text-green-600'
								: 'text-gray-500'}"
					>
						Virtual Host
					</div>
					<div class="text-xs text-gray-500">Select or create</div>
				</div>
			</div>
			<div class="w-16 h-0.5 {currentStep > 2 ? 'bg-green-500' : 'bg-gray-300'} mr-8"></div>

			<!-- Step 3 -->
			<div class="flex items-center">
				<div
					class="w-10 h-10 rounded-full flex items-center justify-center font-semibold {currentStep === 3
						? 'bg-blue-600 text-white'
						: 'bg-gray-300 text-gray-600'}"
				>
					3
				</div>
				<div class="ml-2">
					<div class="text-sm font-medium {currentStep === 3 ? 'text-blue-600' : 'text-gray-500'}">
						Route Details
					</div>
					<div class="text-xs text-gray-500">Configure route</div>
				</div>
			</div>
		</div>
	</div>

	<!-- Step 1: Route Config Selection -->
	{#if currentStep === 1}
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-2">Select Route Configuration</h2>
			<p class="text-sm text-gray-600 mb-6">
				Choose an existing route configuration or create a new one. Route configs group related
				routes together.
			</p>

			<div class="space-y-3 mb-6">
				<!-- Existing Config -->
				<label
					class="flex items-start p-4 border rounded-lg cursor-pointer transition-colors {configMode ===
					'existing'
						? 'border-blue-300 bg-blue-50'
						: 'border-gray-200 hover:border-blue-300 hover:bg-blue-50'}"
				>
					<input
						type="radio"
						name="routeConfig"
						value="existing"
						bind:group={configMode}
						class="mt-1 text-blue-600 focus:ring-blue-500"
					/>
					<div class="ml-3 flex-1">
						<div class="font-medium text-gray-900">Use existing configuration</div>
						<select
							bind:value={selectedConfigName}
							disabled={configMode !== 'existing'}
							class="mt-2 w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 disabled:bg-gray-100"
						>
							{#if routeConfigs.length === 0}
								<option value="">No route configurations available</option>
							{:else}
								{#each routeConfigs as config}
									<option value={config.name}>{config.name}</option>
								{/each}
							{/if}
						</select>
					</div>
				</label>

				<!-- New Config -->
				<label
					class="flex items-start p-4 border rounded-lg cursor-pointer transition-colors {configMode ===
					'new'
						? 'border-blue-300 bg-blue-50'
						: 'border-gray-200 hover:border-blue-300 hover:bg-blue-50'}"
				>
					<input
						type="radio"
						name="routeConfig"
						value="new"
						bind:group={configMode}
						class="mt-1 text-blue-600 focus:ring-blue-500"
					/>
					<div class="ml-3 flex-1">
						<div class="font-medium text-gray-900">Create new configuration</div>
						<p class="text-sm text-gray-500">Start with a fresh route configuration</p>
						{#if configMode === 'new'}
							<div class="mt-3 space-y-3">
								<input
									type="text"
									bind:value={newConfigName}
									placeholder="Configuration name (e.g., payments-api)"
									class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<input
									type="text"
									bind:value={newConfigDescription}
									placeholder="Description (optional)"
									class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<p class="text-xs text-gray-500">
									Name must be lowercase alphanumeric with dashes only
								</p>
							</div>
						{/if}
					</div>
				</label>
			</div>

			<div class="flex justify-end">
				<button
					onclick={nextStep}
					disabled={!canProceedFromStep1()}
					class="px-6 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 disabled:bg-gray-300 disabled:cursor-not-allowed transition-colors"
				>
					Next: Virtual Host
				</button>
			</div>
		</div>
	{/if}

	<!-- Step 2: Virtual Host Selection -->
	{#if currentStep === 2}
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-2">Select Virtual Host</h2>
			<p class="text-sm text-gray-600 mb-6">
				Virtual hosts define the domains for your routes. Select an existing one or create new.
			</p>

			<div class="space-y-3 mb-6">
				<!-- Existing VH -->
				<label
					class="flex items-start p-4 border rounded-lg cursor-pointer transition-colors {vhMode ===
					'existing'
						? 'border-blue-300 bg-blue-50'
						: 'border-gray-200 hover:border-blue-300 hover:bg-blue-50'}"
				>
					<input
						type="radio"
						name="virtualHost"
						value="existing"
						bind:group={vhMode}
						class="mt-1 text-blue-600 focus:ring-blue-500"
					/>
					<div class="ml-3 flex-1">
						<div class="font-medium text-gray-900">Use existing virtual host</div>
						<select
							bind:value={selectedVhName}
							disabled={vhMode !== 'existing'}
							class="mt-2 w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 bg-white disabled:bg-gray-100"
						>
							<option value="">Select virtual host...</option>
							<option value="api-vh">api-vh (api.example.com)</option>
							<option value="internal-vh">internal-vh (internal.example.com)</option>
						</select>
					</div>
				</label>

				<!-- New VH -->
				<label
					class="flex items-start p-4 border rounded-lg cursor-pointer transition-colors {vhMode ===
					'new'
						? 'border-blue-300 bg-blue-50'
						: 'border-gray-200 hover:border-blue-300 hover:bg-blue-50'}"
				>
					<input
						type="radio"
						name="virtualHost"
						value="new"
						bind:group={vhMode}
						class="mt-1 text-blue-600 focus:ring-blue-500"
					/>
					<div class="ml-3 flex-1">
						<div class="font-medium text-gray-900">Create new virtual host</div>
						<p class="text-sm text-gray-500">Define new domains for this route</p>
						{#if vhMode === 'new'}
							<div class="mt-3 space-y-3">
								<input
									type="text"
									bind:value={newVhName}
									placeholder="Virtual host name"
									class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<input
									type="text"
									bind:value={newVhDomains}
									placeholder="Domains (comma-separated, e.g., api.example.com, *.api.example.com)"
									class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<p class="text-xs text-gray-500">
									Name must be lowercase alphanumeric with dashes only
								</p>
							</div>
						{/if}
					</div>
				</label>
			</div>

			<!-- Context Info -->
			<div class="mb-6 p-4 bg-gray-50 rounded-lg border border-gray-200">
				<div class="text-sm text-gray-600">
					<span class="font-medium">Selected Config:</span>
					{finalConfigName}
				</div>
			</div>

			<div class="flex justify-between">
				<button
					onclick={previousStep}
					class="px-6 py-2 text-gray-700 border border-gray-300 rounded-md hover:bg-gray-50 transition-colors"
				>
					<ChevronLeft class="w-4 h-4 inline mr-1" />
					Back
				</button>
				<button
					onclick={nextStep}
					disabled={!canProceedFromStep2()}
					class="px-6 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 disabled:bg-gray-300 disabled:cursor-not-allowed transition-colors"
				>
					Next: Route Details
				</button>
			</div>
		</div>
	{/if}

	<!-- Step 3: Route Details -->
	{#if currentStep === 3}
		<div class="space-y-6">
			<!-- Context Banner -->
			<div class="bg-blue-50 border border-blue-200 rounded-lg p-4">
				<div class="flex items-center gap-4 text-sm">
					<div>
						<span class="text-blue-600 font-medium">Config:</span>
						<span class="text-blue-900">{finalConfigName}</span>
					</div>
					<div class="text-blue-300">|</div>
					<div>
						<span class="text-blue-600 font-medium">Virtual Host:</span>
						<span class="text-blue-900"
							>{vhMode === 'existing' ? selectedVhName : newVhName}</span
						>
					</div>
					<button
						onclick={() => goToStep(2)}
						class="ml-auto text-blue-600 hover:text-blue-800 text-sm transition-colors"
					>
						Change
					</button>
				</div>
			</div>

			<!-- Route Details Form -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Route Details</h2>
				<div class="grid grid-cols-2 gap-6">
					<div class="col-span-2">
						<label class="block text-sm font-medium text-gray-700 mb-1"
							>Route Name <span class="text-red-500">*</span></label
						>
						<input
							type="text"
							bind:value={routeName}
							placeholder="e.g., get-user-profile"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
					</div>
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Match Type</label>
						<select
							bind:value={routePathType}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						>
							<option value="prefix">Prefix</option>
							<option value="exact">Exact</option>
							<option value="template">Template</option>
							<option value="regex">Regex</option>
						</select>
					</div>
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1"
							>Path <span class="text-red-500">*</span></label
						>
						<input
							type="text"
							bind:value={routePath}
							placeholder="/api/v1/users/{'{id}'}"
							class="w-full px-3 py-2 font-mono border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
					</div>
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">HTTP Method</label>
						<select
							bind:value={routeMethod}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						>
							<option value="GET">GET</option>
							<option value="POST">POST</option>
							<option value="PUT">PUT</option>
							<option value="DELETE">DELETE</option>
							<option value="PATCH">PATCH</option>
							<option value="HEAD">HEAD</option>
							<option value="OPTIONS">OPTIONS</option>
						</select>
					</div>
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1"
							>Upstream Cluster <span class="text-red-500">*</span></label
						>
						<select
							bind:value={routeCluster}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						>
							{#if availableClusters.length === 0}
								<option value="">No clusters available</option>
							{:else}
								{#each availableClusters as cluster}
									<option value={cluster.name}>{cluster.name}</option>
								{/each}
							{/if}
						</select>
					</div>
				</div>
			</div>

			<!-- Path Rewrite (Collapsible) -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200">
				<button
					onclick={() => (pathRewriteExpanded = !pathRewriteExpanded)}
					class="w-full px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
				>
					<h2 class="text-lg font-semibold text-gray-900">Path Rewrite</h2>
					<svg
						class="w-5 h-5 text-gray-500 transform transition-transform {pathRewriteExpanded
							? 'rotate-180'
							: ''}"
						fill="none"
						stroke="currentColor"
						viewBox="0 0 24 24"
					>
						<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
					</svg>
				</button>
				{#if pathRewriteExpanded}
					<div class="px-6 pb-6 border-t border-gray-200 pt-4">
						{#if routePathType === 'template'}
							<div>
								<label class="block text-sm font-medium text-gray-700 mb-1">Template Rewrite</label>
								<input
									type="text"
									bind:value={templateRewrite}
									placeholder="/users/{'{user_id}'}"
									class="w-full px-3 py-2 font-mono border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<p class="text-xs text-gray-500 mt-1">
									Rewrite using template pattern (e.g., /api/users/{'{id}'})
								</p>
							</div>
						{:else}
							<div>
								<label class="block text-sm font-medium text-gray-700 mb-1">Prefix Rewrite</label>
								<input
									type="text"
									bind:value={prefixRewrite}
									placeholder="/internal/api"
									class="w-full px-3 py-2 font-mono border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<p class="text-xs text-gray-500 mt-1">Rewrite matched prefix to this value</p>
							</div>
						{/if}
					</div>
				{/if}
			</div>

			<!-- Timeouts & Retries (Collapsible) -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200">
				<button
					onclick={() => (timeoutsExpanded = !timeoutsExpanded)}
					class="w-full px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
				>
					<h2 class="text-lg font-semibold text-gray-900">Timeouts &amp; Retries</h2>
					<svg
						class="w-5 h-5 text-gray-500 transform transition-transform {timeoutsExpanded
							? 'rotate-180'
							: ''}"
						fill="none"
						stroke="currentColor"
						viewBox="0 0 24 24"
					>
						<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
					</svg>
				</button>
				{#if timeoutsExpanded}
					<div class="px-6 pb-6 border-t border-gray-200 pt-4">
						<div class="space-y-4">
							<div>
								<label class="block text-sm font-medium text-gray-700 mb-1"
									>Request Timeout (seconds)</label
								>
								<input
									type="number"
									bind:value={routeTimeout}
									min="1"
									max="300"
									class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
							</div>

							<div class="border-t border-gray-200 pt-4">
								<div class="flex items-center justify-between mb-3">
									<h3 class="text-sm font-medium text-gray-900">Retry Policy</h3>
									<label class="flex items-center cursor-pointer">
										<input
											type="checkbox"
											bind:checked={retryEnabled}
											class="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
										/>
										<span class="ml-2 text-sm text-gray-700">Enable Retries</span>
									</label>
								</div>

								{#if retryEnabled}
									<div class="grid grid-cols-3 gap-4 mb-4">
										<div>
											<label class="block text-sm font-medium text-gray-700 mb-1">Max Retries</label>
											<input
												type="number"
												bind:value={maxRetries}
												min="1"
												max="10"
												class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
											/>
										</div>
										<div>
											<label class="block text-sm font-medium text-gray-700 mb-1"
												>Per-Try Timeout (s)</label
											>
											<input
												type="number"
												bind:value={perTryTimeout}
												min="1"
												max="60"
												class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
											/>
										</div>
									</div>

									<div>
										<label class="block text-sm font-medium text-gray-700 mb-2"
											>Retry On Conditions</label
										>
										<div class="flex flex-wrap gap-2">
											{#each ['5xx', 'reset', 'connect-failure', 'gateway-error', 'retriable-4xx', 'retriable-status-codes'] as condition}
												<label
													class="inline-flex items-center px-3 py-1.5 border rounded cursor-pointer hover:bg-gray-50 transition-colors"
												>
													<input
														type="checkbox"
														checked={retryOn.includes(condition)}
														onchange={(e) => {
															if (e.currentTarget.checked) {
																retryOn = [...retryOn, condition];
															} else {
																retryOn = retryOn.filter((c) => c !== condition);
															}
														}}
														class="rounded text-blue-600"
													/>
													<span class="ml-2 text-sm">{condition}</span>
												</label>
											{/each}
										</div>
									</div>
								{/if}
							</div>
						</div>
					</div>
				{/if}
			</div>

			<!-- MCP Configuration (Collapsible) -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200">
				<button
					onclick={() => (mcpExpanded = !mcpExpanded)}
					class="w-full px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
				>
					<div>
						<h2 class="text-lg font-semibold text-gray-900">MCP Tool Configuration</h2>
						<p class="text-sm text-gray-500">Expose this route as an MCP tool for AI assistants</p>
					</div>
					<label
						class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors {mcpEnabled
							? 'bg-blue-600'
							: 'bg-gray-300'}"
						onclick={(e) => e.stopPropagation()}
					>
						<input type="checkbox" bind:checked={mcpEnabled} class="sr-only" />
						<span
							class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform {mcpEnabled
								? 'translate-x-6'
								: 'translate-x-1'}"
						></span>
					</label>
				</button>
				{#if mcpEnabled}
					<div class="px-6 pb-6 border-t border-gray-200 pt-4 space-y-4">
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Tool Name</label>
							<input
								type="text"
								bind:value={mcpToolName}
								placeholder="get_user_profile"
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
						</div>
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Description</label>
							<textarea
								rows="2"
								bind:value={mcpDescription}
								placeholder="Retrieve user profile information by ID"
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							></textarea>
						</div>
					</div>
				{/if}
			</div>

			<!-- Action Buttons -->
			<div class="flex justify-between">
				<button
					onclick={previousStep}
					disabled={isSubmitting}
					class="px-6 py-2 text-gray-700 border border-gray-300 rounded-md hover:bg-gray-50 disabled:opacity-50 transition-colors"
				>
					<ChevronLeft class="w-4 h-4 inline mr-1" />
					Back
				</button>
				<button
					onclick={handleSubmit}
					disabled={!canSubmit() || isSubmitting}
					class="px-6 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 disabled:bg-gray-300 disabled:cursor-not-allowed transition-colors"
				>
					{isSubmitting ? 'Creating...' : 'Create Route'}
				</button>
			</div>
		</div>
	{/if}
</div>
