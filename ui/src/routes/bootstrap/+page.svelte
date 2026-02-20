<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { bootstrapSchema, type BootstrapSchema } from '$lib/schemas/auth';
	import PasswordStrengthMeter from '$lib/components/PasswordStrengthMeter.svelte';

	let name = $state('');
	let email = $state('');
	let password = $state('');
	let confirmPassword = $state('');
	let errorMessage = $state('');
	let fieldErrors = $state<Record<string, string>>({});
	let isSubmitting = $state(false);
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

	async function handleSubmit(event: Event) {
		event.preventDefault();
		errorMessage = '';
		fieldErrors = {};
		isSubmitting = true;

		try {
			// Validate form
			const formData: BootstrapSchema = { name, email, password, confirmPassword };
			bootstrapSchema.parse(formData);

			// Submit bootstrap request
			const response = await apiClient.bootstrapInitialize({ name, email, password });

			// After successful bootstrap, login with the credentials
			await apiClient.login({ email, password });

			// Set flag to show welcome message
			sessionStorage.setItem('bootstrap_completed', 'true');

			// Redirect to dashboard
			goto('/dashboard');
		} catch (error: any) {
			if (error.errors) {
				// Zod validation errors
				const errors: Record<string, string> = {};
				error.errors.forEach((err: any) => {
					const field = err.path[0];
					errors[field] = err.message;
				});
				fieldErrors = errors;
			} else {
				// API errors
				errorMessage = error instanceof Error ? error.message : 'Bootstrap failed';
			}
		} finally {
			isSubmitting = false;
		}
	}
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
				<p class="mt-2 text-sm text-gray-600">Create your admin account to get started</p>
			</div>

			<div class="bg-white rounded-lg shadow-md p-8">
				<div class="mb-6 p-4 bg-blue-50 rounded-md">
					<p class="text-sm text-blue-800">
						This is your first time setting up Flowplane. Please create an administrator account.
					</p>
				</div>

				<form onsubmit={handleSubmit} class="space-y-6">
					<!-- Name field -->
					<div>
						<label for="name" class="block text-sm font-medium text-gray-700 mb-1">
							Full Name
						</label>
						<input
							id="name"
							name="name"
							type="text"
							autocomplete="name"
							required
							bind:value={name}
							class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
							class:border-red-500={fieldErrors.name}
						/>
						{#if fieldErrors.name}
							<p class="mt-1 text-sm text-red-600">{fieldErrors.name}</p>
						{/if}
					</div>

					<!-- Email field -->
					<div>
						<label for="email" class="block text-sm font-medium text-gray-700 mb-1">
							Email Address
						</label>
						<input
							id="email"
							name="email"
							type="email"
							autocomplete="email"
							required
							bind:value={email}
							class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
							class:border-red-500={fieldErrors.email}
						/>
						{#if fieldErrors.email}
							<p class="mt-1 text-sm text-red-600">{fieldErrors.email}</p>
						{/if}
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
							autocomplete="new-password"
							required
							bind:value={password}
							class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
							class:border-red-500={fieldErrors.password}
						/>
						{#if fieldErrors.password}
							<p class="mt-1 text-sm text-red-600">{fieldErrors.password}</p>
						{/if}
						<PasswordStrengthMeter {password} />
					</div>

					<!-- Confirm Password field -->
					<div>
						<label for="confirmPassword" class="block text-sm font-medium text-gray-700 mb-1">
							Confirm Password
						</label>
						<input
							id="confirmPassword"
							name="confirmPassword"
							type="password"
							autocomplete="new-password"
							required
							bind:value={confirmPassword}
							class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
							class:border-red-500={fieldErrors.confirmPassword}
						/>
						{#if fieldErrors.confirmPassword}
							<p class="mt-1 text-sm text-red-600">{fieldErrors.confirmPassword}</p>
						{/if}
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
							Creating account...
						{:else}
							Create Admin Account
						{/if}
					</button>
				</form>
			</div>

			<p class="text-center text-xs text-gray-500">Flowplane API Gateway - Initial Setup</p>
		</div>
	</div>
{/if}
