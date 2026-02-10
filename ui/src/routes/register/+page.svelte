<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { registerSchema } from '$lib/schemas/auth';
	import { ZodError } from 'zod';
	import type { InviteTokenInfo } from '$lib/api/types';
	import PasswordStrengthMeter from '$lib/components/PasswordStrengthMeter.svelte';

	let token = $state('');
	let tokenInfo = $state<InviteTokenInfo | null>(null);
	let isValidating = $state(true);
	let isSubmitting = $state(false);
	let errorMessage = $state('');
	let fieldErrors = $state<Record<string, string>>({});

	let name = $state('');
	let password = $state('');
	let confirmPassword = $state('');

	let nameInput: HTMLInputElement | undefined = $state();

	onMount(async () => {
		// Extract token from hash fragment using URLSearchParams
		const hash = window.location.hash.substring(1);
		const params = new URLSearchParams(hash);
		let extractedToken = params.get('token');

		// Fallback to sessionStorage if no hash token
		if (!extractedToken) {
			try {
				extractedToken = sessionStorage.getItem('invite_token');
			} catch {
				// Safari private browsing
			}
		}

		if (!extractedToken) {
			isValidating = false;
			errorMessage =
				'This invitation is no longer valid. Please contact your organization administrator for a new invitation.';
			return;
		}

		// Store token in sessionStorage for refresh survival
		try {
			sessionStorage.setItem('invite_token', extractedToken);
		} catch {
			// Safari private browsing may throw on write
		}

		// Clear hash from URL bar to reduce exposure
		window.history.replaceState(null, '', '/register');

		token = extractedToken;

		// Validate the token
		try {
			tokenInfo = await apiClient.validateInviteToken(token);
			isValidating = false;
			// Focus name input after validation
			requestAnimationFrame(() => {
				nameInput?.focus();
			});
		} catch (error: unknown) {
			isValidating = false;
			errorMessage =
				'This invitation is no longer valid. Please contact your organization administrator for a new invitation.';
		}
	});

	async function handleSubmit(event: Event) {
		event.preventDefault();
		errorMessage = '';
		fieldErrors = {};
		isSubmitting = true;

		try {
			// Validate form with Zod
			registerSchema.parse({ name, password, confirmPassword });

			// Accept the invitation
			await apiClient.acceptInvitation({ token, name, password });

			// Clean up stored token
			try {
				sessionStorage.removeItem('invite_token');
			} catch {
				// Safari private browsing
			}

			// Redirect to dashboard (auto-logged in via session cookie)
			await goto('/dashboard');
		} catch (error: unknown) {
			if (error instanceof ZodError) {
				const errors: Record<string, string> = {};
				error.issues.forEach((issue) => {
					const field = issue.path[0];
					if (typeof field === 'string') {
						errors[field] = issue.message;
					}
				});
				fieldErrors = errors;
			} else if (error instanceof Error) {
				const msg = error.message;
				if (msg.includes('Invalid or expired invitation') || msg.includes('revoked')) {
					errorMessage =
						'This invitation was revoked or expired. Please contact your administrator.';
				} else if (msg.includes('already exists')) {
					errorMessage = 'An account with this email already exists. Please log in instead.';
				} else if (msg.includes('429') || msg.includes('Too many')) {
					errorMessage = 'Too many attempts. Please wait a moment and try again.';
				} else {
					errorMessage = msg;
				}
			} else {
				errorMessage = 'Registration failed. Please try again.';
			}
		} finally {
			isSubmitting = false;
		}
	}
</script>

{#if isValidating}
	<div class="min-h-screen bg-gray-50 flex items-center justify-center">
		<div class="text-center" aria-live="polite">
			<div
				class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600 mx-auto mb-4"
			></div>
			<p class="text-gray-600">Validating your invitation...</p>
		</div>
	</div>
{:else if !tokenInfo}
	<div class="min-h-screen bg-gray-50 flex items-center justify-center p-4">
		<div class="max-w-md w-full space-y-8">
			<div class="text-center">
				<h1 class="text-3xl font-bold text-gray-900">Flowplane</h1>
			</div>
			<div class="bg-white rounded-lg shadow-md p-8" aria-live="polite">
				<div class="rounded-md bg-red-50 p-4 mb-6">
					<p class="text-sm text-red-800">{errorMessage}</p>
				</div>
				<a
					href="/login"
					class="w-full flex justify-center py-2 px-4 border border-transparent rounded-md shadow-sm text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
				>
					Go to login
				</a>
			</div>
		</div>
	</div>
{:else}
	<div class="min-h-screen bg-gray-50 flex items-center justify-center p-4">
		<div class="max-w-md w-full space-y-8">
			<div class="text-center">
				<h1 class="text-3xl font-bold text-gray-900">Flowplane</h1>
				<p class="mt-2 text-sm text-gray-600">
					Join {tokenInfo.orgDisplayName} on Flowplane
				</p>
			</div>

			<div class="bg-white rounded-lg shadow-md p-8">
				<div class="mb-6 p-4 bg-blue-50 rounded-md">
					<p class="text-sm text-blue-800">
						You've been invited to join as
						<span class="font-medium"
							>{tokenInfo.role.charAt(0).toUpperCase() + tokenInfo.role.slice(1)}</span
						>.
					</p>
				</div>

				<form onsubmit={handleSubmit} class="space-y-6">
					<!-- Email field (read-only) -->
					<div>
						<label for="email" class="block text-sm font-medium text-gray-700 mb-1">
							Email Address
						</label>
						<input
							id="email"
							name="email"
							type="email"
							autocomplete="email"
							value={tokenInfo.email}
							readonly
							class="w-full px-3 py-2 border border-gray-300 rounded-md bg-gray-50 text-gray-500 cursor-not-allowed"
						/>
					</div>

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
							bind:this={nameInput}
							class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
							class:border-red-500={fieldErrors.name}
						/>
						{#if fieldErrors.name}
							<p class="mt-1 text-sm text-red-600">{fieldErrors.name}</p>
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
						<label
							for="confirmPassword"
							class="block text-sm font-medium text-gray-700 mb-1"
						>
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
						<div class="rounded-md bg-red-50 p-4" aria-live="polite">
							<p class="text-sm text-red-800">
								{errorMessage}
								{#if errorMessage.includes('already exists')}
									<a href="/login" class="underline font-medium hover:text-red-900"
										>Log in</a
									>
								{/if}
							</p>
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
							Create Account
						{/if}
					</button>
				</form>
			</div>

			<p class="text-center text-xs text-gray-500">
				Already have an account?
				<a href="/login" class="text-blue-600 hover:text-blue-800">Sign in</a>
			</p>
		</div>
	</div>
{/if}
