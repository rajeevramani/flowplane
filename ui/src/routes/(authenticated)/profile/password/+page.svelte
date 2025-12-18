<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { z } from 'zod';

	// Zod schema for password change validation
	const passwordChangeSchema = z
		.object({
			currentPassword: z.string().min(1, 'Current password is required'),
			newPassword: z.string().min(8, 'New password must be at least 8 characters'),
			confirmPassword: z.string().min(1, 'Please confirm your new password')
		})
		.refine((data) => data.newPassword === data.confirmPassword, {
			message: 'Passwords do not match',
			path: ['confirmPassword']
		})
		.refine((data) => data.currentPassword !== data.newPassword, {
			message: 'New password must be different from current password',
			path: ['newPassword']
		});

	type PasswordChangeForm = z.infer<typeof passwordChangeSchema>;

	let currentPassword = $state('');
	let newPassword = $state('');
	let confirmPassword = $state('');
	let errorMessage = $state('');
	let successMessage = $state('');
	let fieldErrors = $state<Record<string, string>>({});
	let isSubmitting = $state(false);

	async function handleSubmit(event: Event) {
		event.preventDefault();
		errorMessage = '';
		successMessage = '';
		fieldErrors = {};
		isSubmitting = true;

		try {
			// Validate form with Zod
			const formData: PasswordChangeForm = {
				currentPassword,
				newPassword,
				confirmPassword
			};

			const validated = passwordChangeSchema.parse(formData);

			// Call API to change password
			await apiClient.changePassword({
				currentPassword: validated.currentPassword,
				newPassword: validated.newPassword
			});

			// Success - show message and redirect after delay
			successMessage = 'Password changed successfully! Redirecting to dashboard...';
			currentPassword = '';
			newPassword = '';
			confirmPassword = '';

			setTimeout(() => {
				goto('/dashboard');
			}, 2000);
		} catch (error) {
			// Handle Zod validation errors
			if (error instanceof z.ZodError) {
				const errors: Record<string, string> = {};
				error.issues.forEach((issue) => {
					if (issue.path.length > 0) {
						errors[issue.path[0] as string] = issue.message;
					}
				});
				fieldErrors = errors;
			} else {
				// Handle API errors
				errorMessage = error instanceof Error ? error.message : 'Password change failed';
			}
		} finally {
			isSubmitting = false;
		}
	}
</script>

<div class="max-w-2xl mx-auto">
	<div class="mb-6">
		<h1 class="text-2xl font-bold text-gray-900">Change Password</h1>
		<p class="mt-1 text-sm text-gray-600">
			Update your password to keep your account secure
		</p>
	</div>

	<div class="bg-white rounded-lg shadow p-6">
		<form onsubmit={handleSubmit} class="space-y-6">
			<!-- Current Password field -->
			<div>
				<label for="currentPassword" class="block text-sm font-medium text-gray-700 mb-1">
					Current Password
				</label>
				<input
					id="currentPassword"
					name="currentPassword"
					type="password"
					autocomplete="current-password"
					required
					bind:value={currentPassword}
					class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
					class:border-red-500={fieldErrors.currentPassword}
				/>
				{#if fieldErrors.currentPassword}
					<p class="mt-1 text-sm text-red-600">{fieldErrors.currentPassword}</p>
				{/if}
			</div>

			<!-- New Password field -->
			<div>
				<label for="newPassword" class="block text-sm font-medium text-gray-700 mb-1">
					New Password
				</label>
				<input
					id="newPassword"
					name="newPassword"
					type="password"
					autocomplete="new-password"
					required
					bind:value={newPassword}
					class="w-full px-3 py-2 border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
					class:border-red-500={fieldErrors.newPassword}
				/>
				<p class="mt-1 text-sm text-gray-500">Minimum 8 characters required</p>
				{#if fieldErrors.newPassword}
					<p class="mt-1 text-sm text-red-600">{fieldErrors.newPassword}</p>
				{/if}
			</div>

			<!-- Confirm Password field -->
			<div>
				<label for="confirmPassword" class="block text-sm font-medium text-gray-700 mb-1">
					Confirm New Password
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
					<div class="flex">
						<div class="flex-shrink-0">
							<svg
								class="h-5 w-5 text-red-400"
								viewBox="0 0 20 20"
								fill="currentColor"
								aria-hidden="true"
							>
								<path
									fill-rule="evenodd"
									d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.28 7.22a.75.75 0 00-1.06 1.06L8.94 10l-1.72 1.72a.75.75 0 101.06 1.06L10 11.06l1.72 1.72a.75.75 0 101.06-1.06L11.06 10l1.72-1.72a.75.75 0 00-1.06-1.06L10 8.94 8.28 7.22z"
									clip-rule="evenodd"
								/>
							</svg>
						</div>
						<div class="ml-3">
							<p class="text-sm text-red-800">{errorMessage}</p>
						</div>
					</div>
				</div>
			{/if}

			<!-- Success message -->
			{#if successMessage}
				<div class="rounded-md bg-green-50 p-4">
					<div class="flex">
						<div class="flex-shrink-0">
							<svg
								class="h-5 w-5 text-green-400"
								viewBox="0 0 20 20"
								fill="currentColor"
								aria-hidden="true"
							>
								<path
									fill-rule="evenodd"
									d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z"
									clip-rule="evenodd"
								/>
							</svg>
						</div>
						<div class="ml-3">
							<p class="text-sm text-green-800">{successMessage}</p>
						</div>
					</div>
				</div>
			{/if}

			<!-- Action buttons -->
			<div class="flex gap-3">
				<button
					type="submit"
					disabled={isSubmitting}
					class="flex-1 flex justify-center py-2 px-4 border border-transparent rounded-md shadow-sm text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500 disabled:opacity-50 disabled:cursor-not-allowed"
				>
					{#if isSubmitting}
						Changing password...
					{:else}
						Change Password
					{/if}
				</button>
				<a
					href="/dashboard"
					class="flex-1 flex justify-center py-2 px-4 border border-gray-300 rounded-md shadow-sm text-sm font-medium text-gray-700 bg-white hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
				>
					Cancel
				</a>
			</div>
		</form>
	</div>
</div>
