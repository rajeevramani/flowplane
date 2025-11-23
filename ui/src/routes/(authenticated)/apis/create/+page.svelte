<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type {
		CreateClusterBody,
		CreateRouteBody,
		CreateListenerBody,
		EndpointRequest,
		ListenerResponse,
		HeaderMatchDefinition
	} from '$lib/api/types';
	import EndpointList from '$lib/components/EndpointList.svelte';
	import PathRuleList, { type PathRule } from '$lib/components/PathRuleList.svelte';
	import ListenerSelector, { type ListenerConfig } from '$lib/components/ListenerSelector.svelte';

	// State
	let userTeams = $state<string[]>([]);
	let existingListeners = $state<ListenerResponse[]>([]);
	let isAdmin = $state(false);
	let isSubmitting = $state(false);
	let error = $state<string | null>(null);
	let success = $state<string | null>(null);

	// Form Data
	let apiName = $state('');
	let selectedTeam = $state('');
	let endpoints = $state<EndpointRequest[]>([{ host: '', port: 8080 }]);
	let lbPolicy = $state('ROUND_ROBIN');
	let useTls = $state(false);
	let domain = $state('*');
	let pathRules = $state<PathRule[]>([
		{
			id: crypto.randomUUID(),
			method: '*',
			path: '/',
			pathType: 'prefix',
			headers: [],
			queryParams: []
		}
	]);
	let listenerConfig = $state<ListenerConfig>({
		mode: 'new',
		newAddress: '0.0.0.0',
		newPort: 8080
	});

	onMount(async () => {
		try {
			const session = await apiClient.getSessionInfo();
			isAdmin = session.isAdmin || false;
			const teamsResponse = await apiClient.listTeams();
			userTeams = teamsResponse.teams || [];

			if (!isAdmin && userTeams.length === 1) {
				selectedTeam = userTeams[0];
			}

			// Load existing listeners for the listener selector
			const listeners = await apiClient.listListeners();
			existingListeners = listeners;
		} catch (e) {
			console.error('Failed to load session info:', e);
		}
	});

	// Reload listeners when team changes
	$effect(() => {
		if (selectedTeam) {
			loadListenersForTeam();
		}
	});

	async function loadListenersForTeam() {
		try {
			const listeners = await apiClient.listListeners();
			// Filter listeners by team
			existingListeners = listeners.filter((l) => l.team === selectedTeam);
		} catch (e) {
			console.error('Failed to load listeners:', e);
		}
	}

	function sanitizeName(name: string): string {
		return name
			.toLowerCase()
			.replace(/[^a-z0-9-]/g, '-')
			.replace(/-+/g, '-')
			.replace(/^-|-$/g, '');
	}

	function buildRouteRules() {
		return pathRules.map((rule) => {
			// Build headers array - include :method if not wildcard
			const headers: HeaderMatchDefinition[] = [];
			if (rule.method !== '*') {
				headers.push({ name: ':method', value: rule.method });
			}
			// Add user-defined headers
			headers.push(...rule.headers);

			return {
				name: `${sanitizeName(apiName)}-${rule.method.toLowerCase()}-${sanitizeName(rule.path)}`,
				match: {
					path:
						rule.pathType === 'template'
							? { type: rule.pathType as const, template: rule.path }
							: { type: rule.pathType as const, value: rule.path },
					headers: headers.length > 0 ? headers : undefined,
					queryParameters: rule.queryParams.length > 0 ? rule.queryParams : undefined
				},
				action: {
					type: 'forward' as const,
					cluster: `${sanitizeName(apiName)}-cluster`,
					timeoutSeconds: 15
				}
			};
		});
	}

	async function handleSubmit() {
		error = null;
		success = null;

		// Validation
		if (!apiName.trim()) {
			error = 'API name is required';
			return;
		}
		if (!selectedTeam) {
			error = 'Please select a team';
			return;
		}
		if (endpoints.length === 0 || !endpoints[0].host) {
			error = 'At least one upstream endpoint is required';
			return;
		}
		if (pathRules.length === 0) {
			error = 'At least one path rule is required';
			return;
		}
		if (listenerConfig.mode === 'existing' && !listenerConfig.existingListenerName) {
			error = 'Please select an existing listener or create a new one';
			return;
		}

		isSubmitting = true;

		try {
			const safeName = sanitizeName(apiName);

			// 1. Create Cluster
			const clusterName = `${safeName}-cluster`;
			const clusterBody: CreateClusterBody = {
				team: selectedTeam,
				name: clusterName,
				serviceName: safeName,
				endpoints: endpoints.filter((e) => e.host.trim() !== ''),
				useTls: useTls,
				connectTimeoutSeconds: 5,
				dnsLookupFamily: 'AUTO',
				lbPolicy: endpoints.length > 1 ? (lbPolicy as CreateClusterBody['lbPolicy']) : undefined
			};
			await apiClient.createCluster(clusterBody);

			// 2. Create Route
			const routeName = `${safeName}-routes`;
			const routeBody: CreateRouteBody = {
				team: selectedTeam,
				name: routeName,
				virtualHosts: [
					{
						name: `${safeName}-vhost`,
						domains: [domain],
						routes: buildRouteRules()
					}
				]
			};
			await apiClient.createRoute(routeBody);

			// 3. Create Listener (if new) or note existing
			if (listenerConfig.mode === 'new') {
				const listenerName = `${safeName}-listener`;
				const listenerBody: CreateListenerBody = {
					team: selectedTeam,
					name: listenerName,
					address: listenerConfig.newAddress,
					port: listenerConfig.newPort,
					protocol: 'HTTP',
					filterChains: [
						{
							name: 'default',
							filters: [
								{
									name: 'envoy.filters.network.http_connection_manager',
									filterType: {
										type: 'httpConnectionManager',
										routeConfigName: routeName,
										httpFilters: [{ name: 'envoy.filters.http.router', typedConfig: {} }]
									}
								}
							]
						}
					]
				};
				await apiClient.createListener(listenerBody);
			}

			success = 'API created successfully! Redirecting...';
			setTimeout(() => {
				goto('/apis');
			}, 1500);
		} catch (e: unknown) {
			const message = e instanceof Error ? e.message : 'Failed to create API';
			error = message;
		} finally {
			isSubmitting = false;
		}
	}

	function handleCancel() {
		goto('/apis');
	}
</script>

<div class="max-w-4xl mx-auto">
	<div class="flex items-center gap-4 mb-6">
		<a href="/apis" class="text-blue-600 hover:text-blue-800">
			<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					stroke-width="2"
					d="M10 19l-7-7m0 0l7-7m-7 7h18"
				/>
			</svg>
		</a>
		<h1 class="text-2xl font-bold text-gray-900">Create New API</h1>
	</div>

	<div class="bg-white rounded-lg shadow-md p-6">
		<form
			onsubmit={(e) => {
				e.preventDefault();
				handleSubmit();
			}}
			class="space-y-8"
		>
			<!-- General Info -->
			<div class="space-y-4">
				<h3 class="text-lg font-medium text-gray-900 border-b pb-2">Basic Information</h3>
				<div class="grid grid-cols-1 md:grid-cols-2 gap-6">
					<div>
						<label for="name" class="block text-sm font-medium text-gray-700 mb-1">API Name</label>
						<input
							id="name"
							type="text"
							bind:value={apiName}
							required
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:ring-blue-500 focus:border-blue-500"
							placeholder="e.g. payment-service"
						/>
						<p class="mt-1 text-xs text-gray-500">
							Will be used to name resources (cluster, routes, listener)
						</p>
					</div>

					<div>
						<label for="team" class="block text-sm font-medium text-gray-700 mb-1">Team</label>
						{#if userTeams.length > 1 || isAdmin}
							<select
								id="team"
								bind:value={selectedTeam}
								required
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:ring-blue-500 focus:border-blue-500"
							>
								<option value="">Select a team...</option>
								{#each userTeams as team}
									<option value={team}>{team}</option>
								{/each}
							</select>
						{:else}
							<input
								id="team"
								type="text"
								bind:value={selectedTeam}
								readonly
								class="w-full px-3 py-2 border border-gray-300 rounded-md bg-gray-100"
							/>
						{/if}
					</div>
				</div>
			</div>

			<!-- Upstream Configuration -->
			<div class="space-y-4">
				<h3 class="text-lg font-medium text-gray-900 border-b pb-2">Upstream Service</h3>
				<EndpointList
					{endpoints}
					{lbPolicy}
					onEndpointsChange={(e) => (endpoints = e)}
					onLbPolicyChange={(p) => (lbPolicy = p)}
				/>
				<div class="mt-4">
					<label class="flex items-center">
						<input
							type="checkbox"
							bind:checked={useTls}
							class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
						/>
						<span class="ml-2 text-sm text-gray-700">Use TLS (HTTPS) for upstream connections</span>
					</label>
				</div>
			</div>

			<!-- Path Rules -->
			<div class="space-y-4">
				<h3 class="text-lg font-medium text-gray-900 border-b pb-2">Routing</h3>

				<div>
					<label for="domain" class="block text-sm font-medium text-gray-700 mb-1">Domain</label>
					<input
						id="domain"
						type="text"
						bind:value={domain}
						required
						class="w-full max-w-md px-3 py-2 border border-gray-300 rounded-md focus:ring-blue-500 focus:border-blue-500"
						placeholder="*"
					/>
					<p class="mt-1 text-xs text-gray-500">
						Use * to match all domains, or specify a domain like api.example.com
					</p>
				</div>

				<div class="mt-4">
					<PathRuleList rules={pathRules} onRulesChange={(r) => (pathRules = r)} />
				</div>
			</div>

			<!-- Listener Configuration -->
			<div class="space-y-4">
				<h3 class="text-lg font-medium text-gray-900 border-b pb-2">Listener</h3>
				<ListenerSelector
					listeners={existingListeners}
					config={listenerConfig}
					onConfigChange={(c) => (listenerConfig = c)}
				/>
			</div>

			<!-- Feedback -->
			{#if error}
				<div class="bg-red-50 border-l-4 border-red-500 p-4 rounded-md">
					<p class="text-red-700">{error}</p>
				</div>
			{/if}
			{#if success}
				<div class="bg-green-50 border-l-4 border-green-500 p-4 rounded-md">
					<p class="text-green-700">{success}</p>
				</div>
			{/if}

			<!-- Actions -->
			<div class="flex justify-end gap-4 pt-4 border-t">
				<button
					type="button"
					onclick={handleCancel}
					class="px-6 py-2 border border-gray-300 text-gray-700 font-medium rounded-md hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-gray-500"
				>
					Cancel
				</button>
				<button
					type="submit"
					disabled={isSubmitting}
					class="px-6 py-2 bg-blue-600 text-white font-medium rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500 disabled:opacity-50"
				>
					{isSubmitting ? 'Creating...' : 'Create API'}
				</button>
			</div>
		</form>
	</div>
</div>
