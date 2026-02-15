<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import type { SessionInfoResponse } from '$lib/api/types';
	import StatCard from '$lib/components/StatCard.svelte';
	import AdminResourceSummary from '$lib/components/AdminResourceSummary.svelte';
	import { selectedTeam } from '$lib/stores/team';
	import { adminSummary, adminSummaryLoading, adminSummaryError, getAdminSummary } from '$lib/stores/adminSummary';
	import { isSystemAdmin } from '$lib/stores/org';
	import type { Unsubscriber } from 'svelte/store';

	let isFirstAdmin = $state(false);
	let sessionInfo = $state<SessionInfoResponse | null>(null);
	let currentTeam = $state<string>('');
	let resourceCounts = $state({
		imports: 0,
		listeners: 0,
		routes: 0,
		clusters: 0
	});
	let isLoadingResources = $state(true);
	let unsubscribe: Unsubscriber;

	async function loadResourceCounts(team: string) {
		isLoadingResources = true;
		try {
			const [imports, listeners, routes, clusters] = await Promise.all([
				team
					? apiClient.listImports(team)
					: Promise.resolve([]),
				apiClient.listListeners(),
				apiClient.listRouteConfigs(),
				apiClient.listClusters()
			]);

			resourceCounts = {
				imports: imports.length,
				listeners: listeners.length,
				routes: routes.length,
				clusters: clusters.length
			};
		} catch (error) {
			console.error('Failed to load resource counts:', error);
		} finally {
			isLoadingResources = false;
		}
	}

	onMount(async () => {
		sessionInfo = await apiClient.getSessionInfo();

		// Check if this is the first admin (check if sessionStorage has bootstrap flag)
		const bootstrapCompleted = sessionStorage.getItem('bootstrap_completed');
		if (bootstrapCompleted === 'true') {
			isFirstAdmin = true;
			// Clear the flag so message only shows once
			sessionStorage.removeItem('bootstrap_completed');
		}

		// Platform admin: load admin summary instead of team-filtered resources
		if (sessionInfo.isPlatformAdmin) {
			try {
				await getAdminSummary();
			} catch {
				// Error handled by store
			}
			isLoadingResources = false;
			return;
		}

		// Subscribe to team changes and reload resources
		unsubscribe = selectedTeam.subscribe(async (team) => {
			if (team && team !== currentTeam) {
				currentTeam = team;
				await loadResourceCounts(team);
			}
		});
	});

	onDestroy(() => {
		if (unsubscribe) {
			unsubscribe();
		}
	});
</script>

{#if sessionInfo}
	<!-- First admin welcome message -->
	{#if isFirstAdmin}
		<div class="mb-6 bg-green-50 border-l-4 border-green-500 rounded-md p-6">
			<div class="flex items-start">
				<div class="flex-shrink-0">
					<svg
						class="h-6 w-6 text-green-500"
						fill="none"
						viewBox="0 0 24 24"
						stroke="currentColor"
					>
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							stroke-width="2"
							d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"
						/>
					</svg>
				</div>
				<div class="ml-3">
					<h3 class="text-lg font-medium text-green-800">
						Welcome! Your Flowplane instance is ready
					</h3>
					<div class="mt-2 text-sm text-green-700">
						<p>Your admin account has been created successfully.</p>
						<p class="mt-2">Next steps:</p>
						<ul class="list-disc list-inside mt-2 space-y-1">
							<li>Create additional users and manage teams</li>
							<li>Configure API listeners and routes</li>
							<li>Set up your first API gateway</li>
							<li>Review security and authentication settings</li>
						</ul>
					</div>
				</div>
			</div>
		</div>
	{/if}

	{#if sessionInfo.isPlatformAdmin}
		<!-- Platform Admin Governance Dashboard -->
		<div class="mb-8">
			<h2 class="text-3xl font-bold text-gray-900">Platform Governance</h2>
			<p class="mt-2 text-gray-600">
				Overview of all organizations, teams, and resources across the platform.
			</p>
		</div>

		{#if $adminSummaryLoading}
			<div class="flex items-center justify-center py-12">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
			</div>
		{:else if $adminSummaryError}
			<div class="bg-red-50 border border-red-200 rounded-md p-4 mb-6">
				<p class="text-sm text-red-800">{$adminSummaryError}</p>
			</div>
		{:else if $adminSummary}
			<div class="mb-8">
				<AdminResourceSummary summary={$adminSummary} />
			</div>
		{/if}

		<!-- Governance Quick Actions -->
		<div class="mb-8">
			<h3 class="text-lg font-semibold text-gray-900 mb-4">Quick Actions</h3>
			<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
				<a
					href="/admin/organizations"
					class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-purple-300 hover:shadow-md transition-all"
				>
					<h4 class="text-lg font-semibold text-gray-900 mb-2">Manage Organizations</h4>
					<p class="text-sm text-gray-600">Create tenant orgs and assign org admins</p>
				</a>
				<a
					href="/admin/users"
					class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-purple-300 hover:shadow-md transition-all"
				>
					<h4 class="text-lg font-semibold text-gray-900 mb-2">Manage Users</h4>
					<p class="text-sm text-gray-600">View and manage user accounts across the platform</p>
				</a>
				<a
					href="/admin/audit-log"
					class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-purple-300 hover:shadow-md transition-all"
				>
					<h4 class="text-lg font-semibold text-gray-900 mb-2">Audit Log</h4>
					<p class="text-sm text-gray-600">Review system-wide activity and security events</p>
				</a>
			</div>
		</div>
	{:else}
		<!-- Standard User Dashboard -->

		<!-- Welcome header -->
		<div class="mb-8">
			<h2 class="text-3xl font-bold text-gray-900">Welcome back, {sessionInfo.name}!</h2>
			<p class="mt-2 text-gray-600">
				You have access to {sessionInfo.teams.length} team{sessionInfo.teams.length !== 1
					? 's'
					: ''}.
			</p>
		</div>

		<!-- Team badges -->
		{#if sessionInfo.teams.length > 0}
			<div class="mb-8">
				<h3 class="text-sm font-medium text-gray-700 mb-3">Your Teams</h3>
				<div class="flex flex-wrap gap-2">
					{#each sessionInfo.teams as team}
						<span
							class="inline-flex items-center px-3 py-1 rounded-full text-sm font-medium bg-indigo-100 text-indigo-800"
						>
							{team}
						</span>
					{/each}
				</div>
			</div>
		{/if}

		<!-- Resource Overview Cards -->
		<div class="mb-8">
			<h3 class="text-lg font-semibold text-gray-900 mb-4">Resource Overview</h3>
			{#if isLoadingResources}
				<div class="flex items-center justify-center py-12">
					<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				</div>
			{:else}
				<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
					<StatCard
						title="Imports"
						value={resourceCounts.imports}
						icon="api"
						href="/resources?tab=imports"
						colorClass="blue"
					/>
					<StatCard
						title="Clusters"
						value={resourceCounts.clusters}
						icon="cluster"
						href="/resources?tab=clusters"
						colorClass="purple"
					/>
					<StatCard
						title="Routes"
						value={resourceCounts.routes}
						icon="route"
						href="/resources?tab=routes"
						colorClass="green"
					/>
					<StatCard
						title="Listeners"
						value={resourceCounts.listeners}
						icon="listener"
						href="/resources?tab=listeners"
						colorClass="orange"
					/>
				</div>
			{/if}
		</div>

		<!-- Quick Actions Grid -->
		<div class="mb-8">
			<h3 class="text-lg font-semibold text-gray-900 mb-4">Quick Actions</h3>
			<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
				<!-- Governance admin actions -->
				{#if isSystemAdmin(sessionInfo.scopes)}
					<a
						href="/admin/users"
						class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-purple-300 hover:shadow-md transition-all"
					>
						<h4 class="text-lg font-semibold text-gray-900 mb-2">Manage Users</h4>
						<p class="text-sm text-gray-600">
							Create, edit, and manage user accounts and permissions
						</p>
					</a>

					<a
						href="/admin/audit-log"
						class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-purple-300 hover:shadow-md transition-all"
					>
						<h4 class="text-lg font-semibold text-gray-900 mb-2">View Audit Log</h4>
						<p class="text-sm text-gray-600">
							Review system-wide activity and security events
						</p>
					</a>

					<a
						href="/admin/teams"
						class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-purple-300 hover:shadow-md transition-all"
					>
						<h4 class="text-lg font-semibold text-gray-900 mb-2">Manage Teams</h4>
						<p class="text-sm text-gray-600">
							Create and manage teams, set owners, and control team status
						</p>
					</a>
				{/if}

				<!-- Developer actions -->
				<a
					href="/imports/import"
					class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-blue-300 hover:shadow-md transition-all"
				>
					<h4 class="text-lg font-semibold text-gray-900 mb-2">Import OpenAPI Spec</h4>
					<p class="text-sm text-gray-600">
						Upload and configure your API from an OpenAPI specification
					</p>
				</a>

				<a
					href="/tokens"
					class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-blue-300 hover:shadow-md transition-all"
				>
					<h4 class="text-lg font-semibold text-gray-900 mb-2">Create Token</h4>
					<p class="text-sm text-gray-600">
						Generate personal access tokens for API authentication
					</p>
				</a>

				<a
					href="/dataplanes"
					class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-blue-300 hover:shadow-md transition-all"
				>
					<h4 class="text-lg font-semibold text-gray-900 mb-2">Dataplanes</h4>
					<p class="text-sm text-gray-600">
						Manage dataplanes and download Envoy configuration
					</p>
				</a>
			</div>
		</div>

		<!-- Getting Started Guide -->
		<div class="bg-white rounded-lg shadow-md p-6">
			<h3 class="text-xl font-semibold text-gray-900 mb-4">Getting Started</h3>
			<div class="prose max-w-none">
				<p class="text-gray-600 mb-4">
					Flowplane is an API Gateway platform that helps you manage and secure your APIs. Here's
					how to get started:
				</p>

				<ol class="space-y-4 text-gray-700">
					<li class="flex items-start">
						<span class="flex-shrink-0 w-6 h-6 flex items-center justify-center bg-blue-100 text-blue-800 rounded-full text-sm font-semibold mr-3"
							>1</span
						>
						<div>
							<strong>Import your API:</strong> Upload an OpenAPI specification or manually define
							your API routes, listeners, and clusters.
						</div>
					</li>
					<li class="flex items-start">
						<span class="flex-shrink-0 w-6 h-6 flex items-center justify-center bg-blue-100 text-blue-800 rounded-full text-sm font-semibold mr-3"
							>2</span
						>
						<div>
							<strong>Configure your team:</strong> Set up team-scoped resources and access controls
							for collaboration.
						</div>
					</li>
					<li class="flex items-start">
						<span class="flex-shrink-0 w-6 h-6 flex items-center justify-center bg-blue-100 text-blue-800 rounded-full text-sm font-semibold mr-3"
							>3</span
						>
						<div>
							<strong>Download Envoy config:</strong> Get the Envoy configuration and connect
							your Envoy proxy instances.
						</div>
					</li>
					<li class="flex items-start">
						<span class="flex-shrink-0 w-6 h-6 flex items-center justify-center bg-blue-100 text-blue-800 rounded-full text-sm font-semibold mr-3"
							>4</span
						>
						<div>
							<strong>Monitor and manage:</strong> Use the dashboard to view metrics, manage
							tokens, and review audit logs.
						</div>
					</li>
				</ol>
			</div>
		</div>
	{/if}
{/if}
