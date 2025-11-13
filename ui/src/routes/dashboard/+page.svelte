<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { SessionInfoResponse } from '$lib/api/types';

	let isLoading = $state(true);
	let isFirstAdmin = $state(false);
	let sessionInfo = $state<SessionInfoResponse | null>(null);

	onMount(async () => {
		try {
			sessionInfo = await apiClient.getSessionInfo();
			isLoading = false;

			// Check if this is the first admin (check if sessionStorage has bootstrap flag)
			const bootstrapCompleted = sessionStorage.getItem('bootstrap_completed');
			if (bootstrapCompleted === 'true') {
				isFirstAdmin = true;
				// Clear the flag so message only shows once
				sessionStorage.removeItem('bootstrap_completed');
			}
		} catch (error) {
			// Not authenticated, redirect to login
			goto('/login');
		}
	});

	async function handleLogout() {
		try {
			await apiClient.logout();
			goto('/login');
		} catch (error) {
			console.error('Logout failed:', error);
			// Still redirect to login even if API call fails
			goto('/login');
		}
	}
</script>

{#if isLoading}
	<div class="min-h-screen bg-gray-50 flex items-center justify-center">
		<div class="text-gray-600">Loading...</div>
	</div>
{:else if sessionInfo}
	<div class="min-h-screen bg-gray-50">
		<nav class="bg-white shadow-sm border-b border-gray-200">
			<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
				<div class="flex justify-between h-16 items-center">
					<div class="flex items-center gap-3">
						<h1 class="text-xl font-bold text-gray-900">Flowplane</h1>
						<span class="text-sm text-gray-500">API Gateway Platform</span>
					</div>
					<div class="flex items-center gap-4">
						<!-- User info -->
						<div class="flex items-center gap-2">
							<div class="text-right">
								<div class="text-sm font-medium text-gray-900">{sessionInfo.name}</div>
								<div class="text-xs text-gray-500">{sessionInfo.email}</div>
							</div>
							<!-- Role badge -->
							{#if sessionInfo.isAdmin}
								<span
									class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-purple-100 text-purple-800"
									title="Administrator - Full system access"
								>
									Admin
								</span>
							{:else}
								<span
									class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800"
									title="Developer - Team-scoped access"
								>
									Developer
								</span>
							{/if}
						</div>
						<button
							onclick={handleLogout}
							class="px-4 py-2 text-sm font-medium text-gray-700 hover:text-gray-900 hover:bg-gray-100 rounded-md transition-colors"
						>
							Sign out
						</button>
					</div>
				</div>
			</div>
		</nav>

		<main class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
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

			<!-- Welcome header -->
			<div class="mb-8">
				<h2 class="text-3xl font-bold text-gray-900">Welcome back, {sessionInfo.name}!</h2>
				<p class="mt-2 text-gray-600">
					{#if sessionInfo.isAdmin}
						You have administrator access to the entire system.
					{:else}
						You have access to {sessionInfo.teams.length} team{sessionInfo.teams.length !== 1
							? 's'
							: ''}.
					{/if}
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

			<!-- Quick Actions Grid -->
			<div class="mb-8">
				<h3 class="text-lg font-semibold text-gray-900 mb-4">Quick Actions</h3>
				<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
					<!-- Admin-only actions -->
					{#if sessionInfo.isAdmin}
						<a
							href="/admin/users"
							class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-purple-300 hover:shadow-md transition-all"
						>
							<div class="flex items-start justify-between">
								<div>
									<h4 class="text-lg font-semibold text-gray-900 mb-2">Manage Users</h4>
									<p class="text-sm text-gray-600">
										Create, edit, and manage user accounts and permissions
									</p>
								</div>
								<svg
									class="h-6 w-6 text-purple-500"
									fill="none"
									viewBox="0 0 24 24"
									stroke="currentColor"
								>
									<path
										stroke-linecap="round"
										stroke-linejoin="round"
										stroke-width="2"
										d="M12 4.354a4 4 0 110 5.292M15 21H3v-1a6 6 0 0112 0v1zm0 0h6v-1a6 6 0 00-9-5.197M13 7a4 4 0 11-8 0 4 4 0 018 0z"
									/>
								</svg>
							</div>
						</a>

						<a
							href="/admin/audit-log"
							class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-purple-300 hover:shadow-md transition-all"
						>
							<div class="flex items-start justify-between">
								<div>
									<h4 class="text-lg font-semibold text-gray-900 mb-2">View Audit Log</h4>
									<p class="text-sm text-gray-600">
										Review system-wide activity and security events
									</p>
								</div>
								<svg
									class="h-6 w-6 text-purple-500"
									fill="none"
									viewBox="0 0 24 24"
									stroke="currentColor"
								>
									<path
										stroke-linecap="round"
										stroke-linejoin="round"
										stroke-width="2"
										d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
									/>
								</svg>
							</div>
						</a>
					{/if}

					<!-- Developer actions -->
					<a
						href="/api-definitions/import"
						class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-blue-300 hover:shadow-md transition-all"
					>
						<div class="flex items-start justify-between">
							<div>
								<h4 class="text-lg font-semibold text-gray-900 mb-2">Import OpenAPI Spec</h4>
								<p class="text-sm text-gray-600">
									Upload and configure your API from an OpenAPI specification
								</p>
							</div>
							<svg
								class="h-6 w-6 text-blue-500"
								fill="none"
								viewBox="0 0 24 24"
								stroke="currentColor"
							>
								<path
									stroke-linecap="round"
									stroke-linejoin="round"
									stroke-width="2"
									d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12"
								/>
							</svg>
						</div>
					</a>

					<a
						href="/tokens"
						class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-blue-300 hover:shadow-md transition-all"
					>
						<div class="flex items-start justify-between">
							<div>
								<h4 class="text-lg font-semibold text-gray-900 mb-2">Create Token</h4>
								<p class="text-sm text-gray-600">
									Generate personal access tokens for API authentication
								</p>
							</div>
							<svg
								class="h-6 w-6 text-blue-500"
								fill="none"
								viewBox="0 0 24 24"
								stroke="currentColor"
							>
								<path
									stroke-linecap="round"
									stroke-linejoin="round"
									stroke-width="2"
									d="M15 7a2 2 0 012 2m4 0a6 6 0 01-7.743 5.743L11 17H9v2H7v2H4a1 1 0 01-1-1v-2.586a1 1 0 01.293-.707l5.964-5.964A6 6 0 1121 9z"
								/>
							</svg>
						</div>
					</a>

					<a
						href="/resources"
						class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-blue-300 hover:shadow-md transition-all"
					>
						<div class="flex items-start justify-between">
							<div>
								<h4 class="text-lg font-semibold text-gray-900 mb-2">View Resources</h4>
								<p class="text-sm text-gray-600">
									Browse listeners, routes, clusters, and API definitions
								</p>
							</div>
							<svg
								class="h-6 w-6 text-blue-500"
								fill="none"
								viewBox="0 0 24 24"
								stroke="currentColor"
							>
								<path
									stroke-linecap="round"
									stroke-linejoin="round"
									stroke-width="2"
									d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10"
								/>
							</svg>
						</div>
					</a>

					<a
						href="/bootstrap-config"
						class="block p-6 bg-white rounded-lg border border-gray-200 hover:border-blue-300 hover:shadow-md transition-all"
					>
						<div class="flex items-start justify-between">
							<div>
								<h4 class="text-lg font-semibold text-gray-900 mb-2">
									Download Bootstrap Config
								</h4>
								<p class="text-sm text-gray-600">
									Get Envoy bootstrap configuration for your team
								</p>
							</div>
							<svg
								class="h-6 w-6 text-blue-500"
								fill="none"
								viewBox="0 0 24 24"
								stroke="currentColor"
							>
								<path
									stroke-linecap="round"
									stroke-linejoin="round"
									stroke-width="2"
									d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4"
								/>
							</svg>
						</div>
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
								<strong>Download Envoy config:</strong> Get the bootstrap configuration and connect
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
		</main>
	</div>
{/if}
