<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';

	let errorMessage = $state('');
	let isCheckingStatus = $state(true);

	onMount(async () => {
		try {
			// Check if system needs initialization
			const status = await apiClient.getBootstrapStatus();
			if (!status.needsInitialization) {
				// Already initialized, redirect to login
				goto('/login');
			}
			isCheckingStatus = false;
		} catch (error) {
			errorMessage = 'Failed to check system status';
			isCheckingStatus = false;
		}
	});
</script>

{#if isCheckingStatus}
	<div class="min-h-screen bg-gray-50 flex items-center justify-center">
		<div class="text-gray-600">Checking system status...</div>
	</div>
{:else}
	<div class="min-h-screen bg-gray-50 flex items-center justify-center p-4">
		<div class="max-w-md w-full space-y-8">
			<div class="text-center">
				<h1 class="text-3xl font-bold text-gray-900">Welcome to Flowplane</h1>
				<p class="mt-2 text-sm text-gray-600">Initial setup is managed through Zitadel</p>
			</div>

			<div class="bg-white rounded-lg shadow-md p-8">
				<div class="mb-6 p-4 bg-blue-50 rounded-md">
					<p class="text-sm text-blue-800">
						Flowplane uses Zitadel for identity management. Please complete the initial setup
						in the Zitadel Console, then return here to sign in.
					</p>
				</div>

				{#if errorMessage}
					<div class="rounded-md bg-red-50 p-4 mb-4">
						<p class="text-sm text-red-800">{errorMessage}</p>
					</div>
				{/if}

				<a
					href="/login"
					class="w-full flex justify-center py-2 px-4 border border-transparent rounded-md shadow-sm text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
				>
					Go to Sign In
				</a>
			</div>

			<p class="text-center text-xs text-gray-500">Flowplane API Gateway - Initial Setup</p>
		</div>
	</div>
{/if}
