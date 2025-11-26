<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { CreateTeamRequest, UserResponse } from '$lib/api/types';

	let formData = $state({
		name: '',
		displayName: '',
		description: '',
		ownerUserId: ''
	});

	let users = $state<UserResponse[]>([]);
	let errors = $state<Record<string, string>>({});
	let isSubmitting = $state(false);
	let submitError = $state<string | null>(null);
	let isLoadingUsers = $state(true);

	onMount(async () => {
		// Check authentication and admin access
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!sessionInfo.isAdmin) {
				goto('/dashboard');
				return;
			}

			// Load users for owner dropdown
			await loadUsers();
		} catch (err) {
			goto('/login');
		}
	});

	async function loadUsers() {
		isLoadingUsers = true;
		try {
			const response = await apiClient.listUsers(100, 0);
			users = response.users;
		} catch (err: any) {
			submitError = 'Failed to load users: ' + (err.message || 'Unknown error');
		} finally {
			isLoadingUsers = false;
		}
	}

	function validateForm(): boolean {
		const newErrors: Record<string, string> = {};

		// Name validation (lowercase, alphanumeric with hyphens)
		if (!formData.name.trim()) {
			newErrors.name = 'Team name is required';
		} else if (!/^[a-z0-9-]+$/.test(formData.name)) {
			newErrors.name = 'Team name must be lowercase alphanumeric with hyphens only';
		} else if (formData.name.length > 255) {
			newErrors.name = 'Team name must be 255 characters or less';
		}

		// Display name validation
		if (!formData.displayName.trim()) {
			newErrors.displayName = 'Display name is required';
		} else if (formData.displayName.length > 255) {
			newErrors.displayName = 'Display name must be 255 characters or less';
		}

		// Description validation
		if (formData.description && formData.description.length > 1000) {
			newErrors.description = 'Description must be 1000 characters or less';
		}

		errors = newErrors;
		return Object.keys(newErrors).length === 0;
	}

	async function handleSubmit() {
		if (!validateForm()) {
			return;
		}

		isSubmitting = true;
		submitError = null;

		try {
			const request: CreateTeamRequest = {
				name: formData.name,
				displayName: formData.displayName,
				description: formData.description || null,
				ownerUserId: formData.ownerUserId || null
			};

			const team = await apiClient.adminCreateTeam(request);

			// Navigate to team detail page
			goto(`/admin/teams/${team.id}`);
		} catch (err: any) {
			submitError = err.message || 'Failed to create team';
		} finally {
			isSubmitting = false;
		}
	}

	function handleCancel() {
		goto('/admin/teams');
	}
</script>

<div class="min-h-screen bg-gray-50">
	<!-- Navigation -->
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a
						href="/admin/teams"
						class="text-blue-600 hover:text-blue-800"
						aria-label="Back to teams"
					>
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M10 19l-7-7m0 0l7-7m-7 7h18"
							/>
						</svg>
					</a>
					<h1 class="text-xl font-bold text-gray-900">Create Team</h1>
				</div>
			</div>
		</div>
	</nav>

	<main class="max-w-2xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
		<!-- Error Message -->
		{#if submitError}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{submitError}</p>
			</div>
		{/if}

		<!-- Create Team Form -->
		<div class="bg-white rounded-lg shadow-md p-6">
			<form
				onsubmit={(e) => {
					e.preventDefault();
					handleSubmit();
				}}
			>
				<div class="space-y-6">
					<!-- Team Name -->
					<div>
						<label for="name" class="block text-sm font-medium text-gray-700 mb-2">
							Team Name <span class="text-red-500">*</span>
						</label>
						<input
							id="name"
							type="text"
							bind:value={formData.name}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.name
								? 'border-red-500'
								: ''}"
							placeholder="engineering"
						/>
						{#if errors.name}
							<p class="mt-1 text-sm text-red-600">{errors.name}</p>
						{:else}
							<p class="mt-1 text-xs text-gray-500">
								Immutable identifier (lowercase, alphanumeric with hyphens)
							</p>
						{/if}
					</div>

					<!-- Display Name -->
					<div>
						<label for="displayName" class="block text-sm font-medium text-gray-700 mb-2">
							Display Name <span class="text-red-500">*</span>
						</label>
						<input
							id="displayName"
							type="text"
							bind:value={formData.displayName}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.displayName
								? 'border-red-500'
								: ''}"
							placeholder="Engineering Team"
						/>
						{#if errors.displayName}
							<p class="mt-1 text-sm text-red-600">{errors.displayName}</p>
						{:else}
							<p class="mt-1 text-xs text-gray-500">Human-friendly name (can be changed later)</p>
						{/if}
					</div>

					<!-- Description -->
					<div>
						<label for="description" class="block text-sm font-medium text-gray-700 mb-2">
							Description
						</label>
						<textarea
							id="description"
							bind:value={formData.description}
							rows="3"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.description
								? 'border-red-500'
								: ''}"
							placeholder="Team description..."
						></textarea>
						{#if errors.description}
							<p class="mt-1 text-sm text-red-600">{errors.description}</p>
						{/if}
					</div>

					<!-- Owner -->
					<div>
						<label for="owner" class="block text-sm font-medium text-gray-700 mb-2">
							Team Owner
						</label>
						{#if isLoadingUsers}
							<div class="text-sm text-gray-500">Loading users...</div>
						{:else}
							<select
								id="owner"
								bind:value={formData.ownerUserId}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							>
								<option value="">No owner (optional)</option>
								{#each users as user}
									<option value={user.id}>{user.name} ({user.email})</option>
								{/each}
							</select>
						{/if}
						<p class="mt-1 text-xs text-gray-500">
							Optional: Assign a user as the owner of this team
						</p>
					</div>
				</div>

				<!-- Form Actions -->
				<div class="mt-6 flex justify-end gap-3">
					<button
						type="button"
						onclick={handleCancel}
						disabled={isSubmitting}
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 disabled:opacity-50"
					>
						Cancel
					</button>
					<button
						type="submit"
						disabled={isSubmitting}
						class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50"
					>
						{isSubmitting ? 'Creating...' : 'Create Team'}
					</button>
				</div>
			</form>
		</div>
	</main>
</div>
