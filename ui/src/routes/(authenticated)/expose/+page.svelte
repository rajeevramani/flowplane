<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount } from 'svelte';
	import { Zap, ZapOff, Plus, Server, Radio, Layers } from 'lucide-svelte';
	import type {
		SessionInfoResponse,
		ListenerResponse,
		ClusterResponse,
		RouteResponse,
		ExposeRequest
	} from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import { adminSummary, adminSummaryLoading, adminSummaryError, getAdminSummary } from '$lib/stores/adminSummary';
	import AdminResourceSummary from '$lib/components/AdminResourceSummary.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import { ExposeFormSchema } from '$lib/schemas/expose';

	let sessionInfo = $state<SessionInfoResponse | null>(null);
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let currentTeam = $state<string>('');

	// Data — exposed services are listeners created by expose (naming convention: {name}-listener)
	let listeners = $state<ListenerResponse[]>([]);
	let clusters = $state<ClusterResponse[]>([]);
	let routeConfigs = $state<RouteResponse[]>([]);

	// Expose form state
	let showExposeModal = $state(false);
	let formError = $state<string | null>(null);
	let isSubmitting = $state(false);
	let formName = $state('');
	let formUpstream = $state('');
	let formPort = $state<string>('');
	let formPaths = $state('/');

	// Success feedback
	let successMessage = $state<string | null>(null);

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		if (currentTeam && currentTeam !== value) {
			currentTeam = value;
			loadData();
		} else {
			currentTeam = value;
		}
	});

	onMount(async () => {
		sessionInfo = await apiClient.getSessionInfo();
		if (sessionInfo.isPlatformAdmin) {
			try { await getAdminSummary(); } catch { /* handled by store */ }
			isLoading = false;
			return;
		}
		await loadData();
	});

	async function loadData() {
		isLoading = true;
		error = null;
		successMessage = null;

		try {
			const [listenersData, clustersData, routeConfigsData] = await Promise.all([
				currentTeam ? apiClient.listListeners(currentTeam) : Promise.resolve([]),
				currentTeam ? apiClient.listClusters(currentTeam) : Promise.resolve([]),
				currentTeam ? apiClient.listRouteConfigs(currentTeam) : Promise.resolve([])
			]);

			listeners = listenersData;
			clusters = clustersData;
			routeConfigs = routeConfigsData;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		} finally {
			isLoading = false;
		}
	}

	// Identify exposed services by the naming convention:
	// expose creates: cluster={name}, route_config={name}-routes, listener={name}-listener
	interface ExposedService {
		name: string;
		listener: ListenerResponse;
		cluster: ClusterResponse | undefined;
		routeConfig: RouteResponse | undefined;
		port: number | null;
		upstream: string;
	}

	let exposedServices = $derived.by(() => {
		const services: ExposedService[] = [];
		for (const listener of listeners) {
			// Check if this listener follows the expose naming convention
			if (!listener.name.endsWith('-listener')) continue;
			const baseName = listener.name.replace(/-listener$/, '');

			// Look for matching cluster and route config
			const cluster = clusters.find((c) => c.name === baseName);
			const routeConfig = routeConfigs.find((r) => r.name === `${baseName}-routes`);

			// If we have both a matching cluster and route config, it's an exposed service
			if (cluster && routeConfig) {
				// Extract upstream from cluster endpoints
				let upstream = '';
				const config = cluster.config || {};
				if (Array.isArray(config.endpoints) && config.endpoints.length > 0) {
					const ep = config.endpoints[0];
					if (typeof ep === 'string') {
						upstream = ep;
					} else if (typeof ep === 'object' && ep !== null) {
						const obj = ep as { host?: string; port?: number };
						upstream = obj.host ? `${obj.host}:${obj.port || 80}` : '';
					}
				}

				services.push({
					name: baseName,
					listener,
					cluster,
					routeConfig,
					port: listener.port,
					upstream
				});
			}
		}
		return services;
	});

	function openExposeModal() {
		formName = '';
		formUpstream = '';
		formPort = '';
		formPaths = '/';
		formError = null;
		showExposeModal = true;
	}

	async function handleExpose() {
		formError = null;

		// Parse paths
		const pathsList = formPaths
			.split(',')
			.map((p) => p.trim())
			.filter((p) => p.length > 0);

		// Build request
		const request: ExposeRequest = {
			name: formName.trim(),
			upstream: formUpstream.trim()
		};
		if (formPort.trim()) {
			request.port = parseInt(formPort.trim(), 10);
		}
		if (pathsList.length > 0 && !(pathsList.length === 1 && pathsList[0] === '/')) {
			request.paths = pathsList;
		}

		// Validate with Zod
		const result = ExposeFormSchema.safeParse(request);
		if (!result.success) {
			formError = result.error.issues.map((i) => i.message).join('. ');
			return;
		}

		isSubmitting = true;
		try {
			const response = await apiClient.expose(currentTeam, request);
			showExposeModal = false;
			successMessage = `Service "${response.name}" exposed on port ${response.port}`;
			await loadData();
		} catch (e) {
			formError = e instanceof Error ? e.message : 'Failed to expose service';
		} finally {
			isSubmitting = false;
		}
	}

	async function handleUnexpose(service: ExposedService) {
		if (
			!confirm(
				`Are you sure you want to unexpose "${service.name}"? This will delete the cluster, route config, and listener.`
			)
		) {
			return;
		}

		try {
			await apiClient.unexpose(currentTeam, service.name);
			successMessage = `Service "${service.name}" unexposed`;
			await loadData();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to unexpose service';
		}
	}
</script>

{#if sessionInfo?.isPlatformAdmin}
	<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
		<div class="mb-8">
			<h1 class="text-3xl font-bold text-gray-900">Exposed Services</h1>
			<p class="mt-2 text-sm text-gray-600">
				Platform-wide exposed service summary across all organizations and teams.
			</p>
		</div>
		{#if $adminSummaryLoading}
			<div class="flex items-center justify-center py-12">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
			</div>
		{:else if $adminSummaryError}
			<div class="bg-red-50 border border-red-200 rounded-md p-4">
				<p class="text-sm text-red-800">{$adminSummaryError}</p>
			</div>
		{:else if $adminSummary}
			<AdminResourceSummary summary={$adminSummary} highlightResource="listeners" />
		{/if}
	</div>
{:else}
	<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
		<!-- Header -->
		<div class="mb-8">
			<h1 class="text-3xl font-bold text-gray-900">Exposed Services</h1>
			<p class="mt-2 text-sm text-gray-600">
				Quickly expose backend services through Envoy for the
				<span class="font-medium">{currentTeam}</span> team.
				Each exposed service creates a cluster, route config, and listener automatically.
			</p>
		</div>

		<!-- Action Button -->
		<div class="mb-6">
			<Button onclick={openExposeModal} variant="primary">
				<Plus class="h-4 w-4 mr-2" />
				Expose Service
			</Button>
		</div>

		<!-- Success Message -->
		{#if successMessage}
			<div class="mb-6 bg-green-50 border border-green-200 rounded-md p-4">
				<p class="text-sm text-green-800">{successMessage}</p>
			</div>
		{/if}

		<!-- Loading State -->
		{#if isLoading}
			<div class="flex items-center justify-center py-12">
				<div class="flex flex-col items-center gap-3">
					<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
					<span class="text-sm text-gray-600">Loading exposed services...</span>
				</div>
			</div>
		{:else if error}
			<div class="bg-red-50 border border-red-200 rounded-md p-4">
				<p class="text-sm text-red-800">{error}</p>
			</div>
		{:else if exposedServices.length === 0}
			<!-- Empty State -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
				<Zap class="h-12 w-12 text-gray-400 mx-auto mb-4" />
				<h3 class="text-lg font-medium text-gray-900 mb-2">No exposed services</h3>
				<p class="text-sm text-gray-600 mb-6">
					Expose a backend service to create a cluster, route config, and listener in one step.
				</p>
				<Button onclick={openExposeModal} variant="primary">
					<Plus class="h-4 w-4 mr-2" />
					Expose Service
				</Button>
			</div>
		{:else}
			<!-- Table -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-x-auto">
				<table class="min-w-full divide-y divide-gray-200">
					<thead class="bg-gray-50">
						<tr>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Service Name
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Upstream
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Port
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Resources
							</th>
							<th
								class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Actions
							</th>
						</tr>
					</thead>
					<tbody class="bg-white divide-y divide-gray-200">
						{#each exposedServices as service}
							<tr class="hover:bg-gray-50 transition-colors">
								<td class="px-6 py-4">
									<span class="text-sm font-medium text-gray-900"
										>{service.name}</span
									>
								</td>
								<td class="px-6 py-4">
									<span class="text-sm text-gray-600 font-mono"
										>{service.upstream || 'N/A'}</span
									>
								</td>
								<td class="px-6 py-4">
									<span class="text-sm text-gray-600"
										>{service.port ?? 'N/A'}</span
									>
								</td>
								<td class="px-6 py-4">
									<div class="flex flex-wrap gap-1">
										<Badge variant="blue" size="sm">
											<Server class="h-3 w-3 mr-1" />
											Cluster
										</Badge>
										<Badge variant="green" size="sm">
											<Layers class="h-3 w-3 mr-1" />
											Routes
										</Badge>
										<Badge variant="orange" size="sm">
											<Radio class="h-3 w-3 mr-1" />
											Listener
										</Badge>
									</div>
								</td>
								<td class="px-6 py-4 text-right">
									<button
										onclick={() => handleUnexpose(service)}
										class="inline-flex items-center gap-1 px-3 py-1.5 text-sm text-red-600 hover:bg-red-50 rounded-md transition-colors"
										title="Unexpose service"
									>
										<ZapOff class="h-4 w-4" />
										Unexpose
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</div>
{/if}

<!-- Expose Modal -->
<Modal
	show={showExposeModal}
	title="Expose Service"
	onClose={() => (showExposeModal = false)}
	onConfirm={handleExpose}
	confirmText={isSubmitting ? 'Exposing...' : 'Expose'}
>
	{#snippet children()}
		<div class="space-y-4">
			{#if formError}
				<div class="bg-red-50 border border-red-200 rounded-md p-3">
					<p class="text-sm text-red-800">{formError}</p>
				</div>
			{/if}

			<div>
				<label for="expose-name" class="block text-sm font-medium text-gray-700 mb-1"
					>Service Name</label
				>
				<input
					id="expose-name"
					type="text"
					bind:value={formName}
					placeholder="my-api"
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 text-sm"
				/>
				<p class="mt-1 text-xs text-gray-500">
					Alphanumeric, hyphens, dots, underscores. Used as the cluster name.
				</p>
			</div>

			<div>
				<label for="expose-upstream" class="block text-sm font-medium text-gray-700 mb-1"
					>Upstream URL</label
				>
				<input
					id="expose-upstream"
					type="text"
					bind:value={formUpstream}
					placeholder="http://localhost:8080"
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 text-sm"
				/>
				<p class="mt-1 text-xs text-gray-500">Backend service address in host:port format.</p>
			</div>

			<div>
				<label for="expose-port" class="block text-sm font-medium text-gray-700 mb-1"
					>Listener Port <span class="text-gray-400">(optional)</span></label
				>
				<input
					id="expose-port"
					type="number"
					bind:value={formPort}
					placeholder="Auto-assigned (10001-10020)"
					min="10001"
					max="10020"
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 text-sm"
				/>
				<p class="mt-1 text-xs text-gray-500">
					Leave empty for auto-assignment from pool 10001-10020.
				</p>
			</div>

			<div>
				<label for="expose-paths" class="block text-sm font-medium text-gray-700 mb-1"
					>Paths <span class="text-gray-400">(optional)</span></label
				>
				<input
					id="expose-paths"
					type="text"
					bind:value={formPaths}
					placeholder="/"
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 text-sm"
				/>
				<p class="mt-1 text-xs text-gray-500">
					Comma-separated paths to route. Defaults to "/".
				</p>
			</div>
		</div>
	{/snippet}
</Modal>
