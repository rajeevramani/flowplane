<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';

	let email = $state('');
	let password = $state('');
	let errorMessage = $state('');
	let isSubmitting = $state(false);

	async function handleSubmit(event: Event) {
		event.preventDefault();
		errorMessage = '';
		isSubmitting = true;

		try {
			await apiClient.login({ email, password });
			// Login successful, redirect to dashboard
			goto('/dashboard');
		} catch (error) {
			// Error handling - show error message
			errorMessage = error instanceof Error ? error.message : 'Login failed';
		} finally {
			isSubmitting = false;
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
			<form onsubmit={handleSubmit} class="space-y-6">
				<!-- Email field -->
				<div>
					<label for="email" class="block text-sm font-medium text-gray-700 mb-1">
						Email address
					</label>
					<input
						id="email"
						name="email"
						type="email"
						autocomplete="email"
						required
						bind:value={email}
						class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
					/>
				</div>

				<!-- Password field -->
				<div>
					<label for="password" class="block text-sm font-medium text-gray-700 mb-1">
						Password
					</label>
					<input
						id="password"
						name="password"
						type="password"
						autocomplete="current-password"
						required
						bind:value={password}
						class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
					/>
				</div>

				<!-- Error message -->
				{#if errorMessage}
					<div class="rounded-md bg-red-50 p-4">
						<p class="text-sm text-red-800">{errorMessage}</p>
					</div>
				{/if}

				<!-- Submit button -->
				<button
					type="submit"
					disabled={isSubmitting}
					class="w-full flex justify-center py-2 px-4 border border-transparent rounded-md shadow-sm text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500 disabled:opacity-50 disabled:cursor-not-allowed"
				>
					{#if isSubmitting}
						Signing in...
					{:else}
						Sign in
					{/if}
				</button>
			</form>
		</div>

		<p class="text-center text-xs text-gray-500">
			Flowplane API Gateway - Session-based Authentication
		</p>
	</div>
</div>
