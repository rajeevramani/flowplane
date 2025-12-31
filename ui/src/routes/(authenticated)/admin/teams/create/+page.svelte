<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { CreateTeamRequest, UserResponse } from '$lib/api/types';
	import { ErrorAlert, FormActions, PageHeader } from '$lib/components/forms';
	import { validateRequired, validateMaxLength, runValidators } from '$lib/utils/validators';

	let formData = $state({
		name: '',
		displayName: '',
		description: '',
		ownerUserId: ''
	});

	let users = $state<UserResponse[]>([]);
	let errors = $state<Record<string, string>>({});
	let isSubmitting = $state(false);
	let error = $state<string | null>(null);
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
		} catch (err: unknown) {
			error = 'Failed to load users: ' + (err instanceof Error ? err.message : 'Unknown error');
		} finally {
			isLoadingUsers = false;
		}
	}

	function validateForm(): boolean {
		const newErrors: Record<string, string> = {};

		// Name validation (lowercase, alphanumeric with hyphens)
		const nameError = runValidators([
			() => validateRequired(formData.name, 'Team name'),
			() => validateMaxLength(formData.name, 255, 'Team name')
		]);
		if (nameError) {
			newErrors.name = nameError;
		} else if (!/^[a-z0-9-]+$/.test(formData.name)) {
			newErrors.name = 'Team name must be lowercase alphanumeric with hyphens only';
		}

		// Display name validation
		const displayNameError = runValidators([
			() => validateRequired(formData.displayName, 'Display name'),
			() => validateMaxLength(formData.displayName, 255, 'Display name')
		]);
		if (displayNameError) {
			newErrors.displayName = displayNameError;
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
		error = null;

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
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to create team';
		} finally {
			isSubmitting = false;
		}
	}

	function handleCancel() {
		goto('/admin/teams');
	}
</script>

<div class="max-w-2xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
	<!-- Page Header with Back Button -->
	<PageHeader title="Create Team" onBack={handleCancel} />

	<!-- Error Message -->
	<ErrorAlert message={error} />

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
		</form>
	</div>

	<!-- Action Buttons -->
	<FormActions
		{isSubmitting}
		submitLabel="Create Team"
		submittingLabel="Creating..."
		onSubmit={handleSubmit}
		onCancel={handleCancel}
	/>
</div>
