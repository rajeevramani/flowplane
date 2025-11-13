<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	let isLoading = $state(true);

	onMount(async () => {
		try {
			await apiClient.getSessionInfo();
			isLoading = false;
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
