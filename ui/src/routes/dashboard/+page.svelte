<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	let isLoading = $state(true);
	let isFirstAdmin = $state(false);

	onMount(async () => {
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			isLoading = false;

			// Check if this is the first admin (check if localStorage has bootstrap flag)
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
{:else}
	<div class="min-h-screen bg-gray-50">
		<nav class="bg-white shadow-sm">
			<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
				<div class="flex justify-between h-16 items-center">
					<div>
						<h1 class="text-xl font-bold text-gray-900">Flowplane Dashboard</h1>
					</div>
					<button
						onclick={handleLogout}
						class="px-4 py-2 text-sm font-medium text-gray-700 hover:text-gray-900 hover:bg-gray-100 rounded-md"
					>
						Sign out
					</button>
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

			<div class="bg-white rounded-lg shadow-md p-6">
				<h2 class="text-2xl font-semibold text-gray-900 mb-4">Welcome to Flowplane</h2>
				<p class="text-gray-600">
					You are successfully authenticated! This is a protected page that requires a valid session.
				</p>

				<div class="mt-6 p-4 bg-blue-50 rounded-md">
					<h3 class="text-sm font-medium text-blue-800 mb-2">Session Information</h3>
					<p class="text-sm text-blue-700">
						Your session is active and CSRF token is stored in sessionStorage.
					</p>
				</div>
			</div>
		</main>
	</div>
{/if}
