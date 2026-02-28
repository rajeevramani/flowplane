<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { userManager } from '$lib/auth/oidc-config';

	let errorMessage = $state('');

	onMount(async () => {
		try {
			await userManager.signinRedirectCallback();
			await goto('/dashboard');
		} catch (error: unknown) {
			errorMessage =
				error instanceof Error ? error.message : 'Authentication failed. Please try again.';
		}
	});
</script>

{#if errorMessage}
	<div class="min-h-screen bg-gray-50 flex items-center justify-center p-4">
		<div class="max-w-md w-full space-y-8">
			<div class="text-center">
				<h1 class="text-3xl font-bold text-gray-900">Flowplane</h1>
			</div>
			<div class="bg-white rounded-lg shadow-md p-8">
				<div class="rounded-md bg-red-50 p-4 mb-6">
					<p class="text-sm text-red-800">{errorMessage}</p>
				</div>
				<a
					href="/login"
					class="w-full flex justify-center py-2 px-4 border border-transparent rounded-md shadow-sm text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
				>
					Back to login
				</a>
			</div>
		</div>
	</div>
{:else}
	<div class="min-h-screen bg-gray-50 flex items-center justify-center">
		<div class="text-center">
			<div
				class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600 mx-auto mb-4"
			></div>
			<p class="text-gray-600">Completing sign in...</p>
		</div>
	</div>
{/if}
