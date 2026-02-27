<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Plus, ChevronDown, ChevronUp } from 'lucide-svelte';
	import type { ClusterResponse, CreateRouteBody, RouteResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import {
		VirtualHostEditor,
		JsonPanel,
		WizardCreateFlow,
		type VirtualHostFormState,
		type RouteFormState
	} from '$lib/components/route-config';
	import { ErrorAlert, FormActions, PageHeader } from '$lib/components/forms';
	import { validateRequired, runValidators } from '$lib/utils/validators';

	interface FormState {
		name: string;
		team: string;
		virtualHosts: VirtualHostFormState[];
	}

	let currentTeam = $state<string>('');
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let isSubmitting = $state(false);
	let clusters = $state<ClusterResponse[]>([]);
	let routeConfigs = $state<RouteResponse[]>([]);
	let advancedExpanded = $state(false);
	let activeTab = $state<'configuration' | 'json'>('configuration');
	let createApproach = $state<'wizard' | 'single-page' | 'smart-defaults'>('wizard');

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Initialize form state
	let formState = $state<FormState>({
		name: '',
		team: currentTeam,
		virtualHosts: [
			{
				id: `vh-${Date.now()}`,
				name: 'vh-1',
				domains: [],
				routes: []
			}
		]
	});

	// Load clusters and route configs
	onMount(async () => {
		try {
			const [clustersData, routeConfigsData] = await Promise.all([
				apiClient.listClusters(),
				apiClient.listRouteConfigs()
			]);
			clusters = clustersData;
			routeConfigs = routeConfigsData;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	});

	// Get available cluster names
	let availableClusters = $derived(clusters.map((c) => c.name));

	// Build JSON payload from form state
	let jsonPayload = $derived(buildRouteConfigJSON(formState));

	function buildRouteConfigJSON(form: FormState): string {
		const payload: any = {
			team: form.team || currentTeam,
			name: form.name || '',
			virtualHosts: form.virtualHosts.map((vh) => ({
				name: vh.name,
				domains: vh.domains,
				routes: vh.routes.map((r) => {
					const action: any = {
						type: 'forward' as const,
						cluster: r.cluster,
						timeoutSeconds: r.timeout || 30
					};

					// Add path rewrites if specified
					if (r.prefixRewrite) {
						action.prefixRewrite = r.prefixRewrite;
					}
					if (r.templateRewrite) {
						action.templateRewrite = r.templateRewrite;
					}

					// Add retry policy if enabled
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
							path: r.pathType === 'template'
								? { type: r.pathType, template: r.path }
								: { type: r.pathType, value: r.path },
							headers: r.method && r.method !== '*' ? [{ name: ':method', value: r.method }] : []
						},
						action
					};
				})
			}))
		};

		return JSON.stringify(payload, null, 2);
	}

	// Add virtual host
	function handleAddVirtualHost() {
		const vhNumber = formState.virtualHosts.length + 1;
		formState.virtualHosts = [
			...formState.virtualHosts,
			{
				id: `vh-${Date.now()}`,
				name: `vhost-${vhNumber}`,
				domains: [],
				routes: []
			}
		];
	}

	// Remove virtual host
	function handleRemoveVirtualHost(index: number) {
		formState.virtualHosts = formState.virtualHosts.filter((_, i) => i !== index);
	}

	// Update virtual host
	function handleUpdateVirtualHost(index: number, updated: VirtualHostFormState) {
		formState.virtualHosts = formState.virtualHosts.map((vh, i) =>
			i === index ? updated : vh
		);
	}

	// Validate form using reusable validators
	function validateForm(): string | null {
		// Basic validation
		const basicError = runValidators([
			() => validateRequired(formState.name, 'Configuration name')
		]);
		if (basicError) return basicError;

		// Pattern validation
		if (!/^[a-z0-9-]+$/.test(formState.name))
			return 'Name must be lowercase alphanumeric with dashes';

		// Virtual hosts validation
		if (formState.virtualHosts.length === 0) return 'At least one virtual host is required';

		for (const vh of formState.virtualHosts) {
			if (vh.domains.length === 0) return `Virtual host "${vh.name}" must have at least one domain`;
			if (vh.routes.length === 0) return `Virtual host "${vh.name}" must have at least one route`;
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
			await apiClient.createRouteConfig(payload);
			goto('/route-configs');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to create configuration';
		} finally {
			isSubmitting = false;
		}
	}

	// Handle cancel
	function handleCancel() {
		goto('/route-configs');
	}

	// Handle wizard completion
	async function handleWizardComplete(formData: CreateRouteBody) {
		error = null;
		isSubmitting = true;

		try {
			const payload = { ...formData, team: currentTeam };
			await apiClient.createRouteConfig(payload);
			goto('/route-configs');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to create configuration';
			isSubmitting = false;
		}
	}

	// Get route config summaries for wizard
	let routeConfigSummaries = $derived(
		routeConfigs.map((rc) => ({
			name: rc.name,
			id: rc.name
		}))
	);
</script>

<div class="min-h-screen bg-gray-50">
	<div class="px-8 py-8">
		<!-- Page Header -->
		<PageHeader
			title="Create Route Configuration"
			subtitle="Define a new route configuration with virtual hosts and routes"
			onBack={handleCancel}
		/>

		<!-- Error Message -->
		<ErrorAlert message={error} />

		<!-- Approach Selector Tabs -->
		<div class="mb-6">
			<div class="border-b border-gray-200">
				<nav class="-mb-px flex space-x-1" aria-label="Create Approach">
					<button
						onclick={() => (createApproach = 'wizard')}
						class="{createApproach === 'wizard'
							? 'border-blue-500 text-blue-600'
							: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'} whitespace-nowrap py-3 px-4 border-b-2 font-medium text-sm transition-colors"
					>
						Wizard
					</button>
					<button
						onclick={() => (createApproach = 'single-page')}
						class="{createApproach === 'single-page'
							? 'border-blue-500 text-blue-600'
							: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'} whitespace-nowrap py-3 px-4 border-b-2 font-medium text-sm transition-colors"
					>
						Single Page
					</button>
					<button
						onclick={() => (createApproach = 'smart-defaults')}
						class="{createApproach === 'smart-defaults'
							? 'border-blue-500 text-blue-600'
							: 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300'} whitespace-nowrap py-3 px-4 border-b-2 font-medium text-sm transition-colors"
						disabled
						title="Coming soon"
					>
						Smart Defaults
						<span class="ml-1 text-xs">(Coming Soon)</span>
					</button>
				</nav>
			</div>
		</div>

		<!-- Approach Content -->
		{#if createApproach === 'wizard'}
			<!-- Wizard Approach -->
			<WizardCreateFlow
				availableClusters={clusters}
				routeConfigs={routeConfigSummaries}
				onComplete={handleWizardComplete}
				onCancel={handleCancel}
			/>
		{:else if createApproach === 'single-page'}
			<!-- Single Page Approach (existing form) -->

			<!-- Basic Information -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Basic Information</h2>
				<div class="space-y-4">
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Configuration Name <span class="text-red-500">*</span>
						</label>
						<input
							type="text"
							bind:value={formState.name}
							placeholder="e.g., user-service-routes"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<p class="text-xs text-gray-500 mt-1">
							A unique name to identify this route configuration (lowercase, alphanumeric, dashes only)
						</p>
					</div>
				</div>
			</div>

			<!-- Virtual Hosts -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
				<div class="flex items-center justify-between mb-4">
					<div>
						<h2 class="text-lg font-semibold text-gray-900">Virtual Hosts</h2>
						<p class="text-sm text-gray-600">
							Each virtual host groups domains and their associated routes together
						</p>
					</div>
					<button
						onclick={handleAddVirtualHost}
						class="px-3 py-1.5 text-sm text-blue-600 border border-blue-600 rounded-md hover:bg-blue-50 transition-colors"
					>
						<Plus class="h-4 w-4 inline mr-1" />
						Add Virtual Host
					</button>
				</div>

				<div class="space-y-4">
					{#each formState.virtualHosts as vh, index}
						<VirtualHostEditor
							virtualHost={vh}
							{index}
							canRemove={formState.virtualHosts.length > 1}
							onUpdate={(updated) => handleUpdateVirtualHost(index, updated)}
							onRemove={() => handleRemoveVirtualHost(index)}
							{availableClusters}
							routeConfigName=""
						/>
					{/each}
				</div>

				{#if formState.virtualHosts.length === 0}
					<div class="border-2 border-dashed border-gray-300 rounded-lg p-8 text-center">
						<p class="text-sm text-gray-600 mb-3">No virtual hosts defined</p>
						<button
							onclick={handleAddVirtualHost}
							class="px-4 py-2 text-sm text-blue-600 border border-blue-600 rounded-md hover:bg-blue-50 transition-colors"
						>
							<Plus class="h-4 w-4 inline mr-1" />
							Add Virtual Host
						</button>
					</div>
				{/if}

				<div class="mt-4 bg-blue-50 border border-blue-200 rounded-md p-3 text-sm text-blue-800">
					<strong>Note:</strong> Each virtual host groups domains and their routes together. Routes defined
					in a virtual host will only apply to the domains listed in that virtual host.
				</div>
			</div>

			<!-- Advanced (Collapsible) -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 mb-6">
				<button
					onclick={() => (advancedExpanded = !advancedExpanded)}
					class="w-full px-6 py-4 flex items-center justify-between hover:bg-gray-50 transition-colors"
				>
					<h2 class="text-lg font-semibold text-gray-900">Advanced Settings</h2>
					{#if advancedExpanded}
						<ChevronUp class="w-5 h-5 text-gray-500" />
					{:else}
						<ChevronDown class="w-5 h-5 text-gray-500" />
					{/if}
				</button>
				{#if advancedExpanded}
					<div class="px-6 pb-6">
						<p class="text-sm text-gray-600">Advanced settings will be added in future updates.</p>
					</div>
				{/if}
			</div>

			<!-- Action Buttons -->
			<FormActions
				{isSubmitting}
				submitLabel="Create Configuration"
				submittingLabel="Creating..."
				onSubmit={handleSubmit}
				onCancel={handleCancel}
			/>
		{:else}
			<!-- Smart Defaults Approach (coming soon) -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
				<div class="max-w-md mx-auto">
					<div class="w-16 h-16 bg-blue-100 rounded-full flex items-center justify-center mx-auto mb-4">
						<svg
							class="w-8 h-8 text-blue-600"
							fill="none"
							stroke="currentColor"
							viewBox="0 0 24 24"
						>
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 117.072 0l-.548.547A3.374 3.374 0 0014 18.469V19a2 2 0 11-4 0v-.531c0-.895-.356-1.754-.988-2.386l-.548-.547z"
							/>
						</svg>
					</div>
					<h3 class="text-xl font-semibold text-gray-900 mb-2">Smart Defaults Coming Soon</h3>
					<p class="text-gray-600">
						Auto-detection of route configuration and virtual host based on domain and path will be
						available in a future update.
					</p>
				</div>
			</div>
		{/if}
	</div>
</div>
