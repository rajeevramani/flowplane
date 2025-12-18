<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Plus, ChevronDown, ChevronUp, ArrowLeft } from 'lucide-svelte';
	import type { ClusterResponse, CreateRouteBody } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import VirtualHostEditor, {
		type VirtualHostFormState,
		type RouteFormState
	} from '$lib/components/route-config/VirtualHostEditor.svelte';
	import JsonPanel from '$lib/components/route-config/JsonPanel.svelte';
	import Button from '$lib/components/Button.svelte';

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
	let advancedExpanded = $state(false);
	let activeTab = $state<'configuration' | 'json'>('configuration');

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

	// Load clusters
	onMount(async () => {
		try {
			clusters = await apiClient.listClusters();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load clusters';
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

	// Validate form
	function validateForm(): string | null {
		if (!formState.name) return 'Configuration name is required';
		if (!/^[a-z0-9-]+$/.test(formState.name))
			return 'Name must be lowercase alphanumeric with dashes';
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
						<h1 class="text-3xl font-bold text-gray-900">Create Route Configuration</h1>
						<p class="text-sm text-gray-600 mt-1">
							Define a new route configuration with virtual hosts and routes
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
			<div class="sticky bottom-0 bg-white border-t border-gray-200 p-4 -mx-8 flex justify-end gap-3">
				<Button onclick={handleCancel} variant="secondary" disabled={isSubmitting}>
					Cancel
				</Button>
				<Button onclick={handleSubmit} variant="primary" disabled={isSubmitting}>
					{isSubmitting ? 'Creating...' : 'Create Configuration'}
				</Button>
			</div>
		{:else}
			<!-- JSON Tab -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<JsonPanel jsonString={jsonPayload} editable={false} />
			</div>
		{/if}
	</div>
</div>
