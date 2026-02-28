<script lang="ts">
	import { userManager } from '$lib/auth/oidc-config';

	let errorMessage = $state('');
	let isRedirecting = $state(false);

	async function handleSignIn() {
		errorMessage = '';
		isRedirecting = true;

		try {
			await userManager.signinRedirect();
		} catch (error: unknown) {
			errorMessage =
				error instanceof Error ? error.message : 'Failed to start sign in. Please try again.';
			isRedirecting = false;
		}
	}
</script>

<div class="min-h-screen bg-gray-50 flex items-center justify-center p-4">
	<div class="max-w-md w-full space-y-8">
		<div class="text-center">
			<h1 class="text-3xl font-bold text-gray-900">Flowplane</h1>
			<p class="mt-2 text-sm text-gray-600">Sign in to your account</p>
		</div>

		<div class="bg-white rounded-lg shadow-md p-8">
			<!-- Error message -->
			{#if errorMessage}
				<div class="rounded-md bg-red-50 p-4 mb-6">
					<p class="text-sm text-red-800">{errorMessage}</p>
				</div>
			{/if}

			<!-- Sign in button -->
			<button
				onclick={handleSignIn}
				disabled={isRedirecting}
				class="w-full flex justify-center py-2 px-4 border border-transparent rounded-md shadow-sm text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500 disabled:opacity-50 disabled:cursor-not-allowed"
			>
				{#if isRedirecting}
					Redirecting...
				{:else}
					Sign in with Zitadel
				{/if}
			</button>
		</div>

		<p class="text-center text-xs text-gray-500">
			Need an account? Contact your organization administrator.
		</p>
	</div>
</div>
