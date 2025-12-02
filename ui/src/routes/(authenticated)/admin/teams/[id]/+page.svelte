<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import type { TeamResponse, UpdateTeamRequest, UserResponse, TeamStatus } from '$lib/api/types';

	let teamId = $derived($page.params.id);

	let team = $state<TeamResponse | null>(null);
	let users = $state<UserResponse[]>([]);
	let isLoading = $state(true);
	let isLoadingUsers = $state(true);
	let isEditing = $state(false);
	let isSubmitting = $state(false);
	let isDeleting = $state(false);
	let error = $state<string | null>(null);
	let submitError = $state<string | null>(null);
	let showDeleteConfirm = $state(false);

	let formData = $state({
		displayName: '',
		description: '',
		ownerUserId: '',
		status: 'active' as TeamStatus
	});

	let errors = $state<Record<string, string>>({});

	onMount(async () => {
		// Check authentication and admin access
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!sessionInfo.isAdmin) {
				goto('/dashboard');
				return;
			}

			await Promise.all([loadTeam(), loadUsers()]);
		} catch (err) {
			goto('/login');
		}
	});

	async function loadTeam() {
		isLoading = true;
		error = null;

		try {
			team = await apiClient.adminGetTeam(teamId);
			// Initialize form with team data
			formData.displayName = team.displayName;
			formData.description = team.description || '';
			formData.ownerUserId = team.ownerUserId || '';
			formData.status = team.status;
		} catch (err: any) {
			error = err.message || 'Failed to load team';
		} finally {
			isLoading = false;
		}
	}

	async function loadUsers() {
		isLoadingUsers = true;
		try {
			const response = await apiClient.listUsers(100, 0);
			users = response.users;
		} catch (err: any) {
			// Non-fatal error, just log
			console.error('Failed to load users:', err);
		} finally {
			isLoadingUsers = false;
		}
	}

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function getStatusColor(status: TeamStatus): string {
		switch (status) {
			case 'active':
				return 'bg-green-100 text-green-800';
			case 'suspended':
				return 'bg-yellow-100 text-yellow-800';
			case 'archived':
				return 'bg-gray-100 text-gray-800';
			default:
				return 'bg-gray-100 text-gray-800';
		}
	}

	function validateForm(): boolean {
		const newErrors: Record<string, string> = {};

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

	async function handleUpdate() {
		if (!validateForm()) {
			return;
		}

		isSubmitting = true;
		submitError = null;

		try {
			const request: UpdateTeamRequest = {
				displayName: formData.displayName,
				description: formData.description || null,
				ownerUserId: formData.ownerUserId || null,
				status: formData.status
			};

			const updatedTeam = await apiClient.adminUpdateTeam(teamId, request);
			team = updatedTeam;
			isEditing = false;
		} catch (err: any) {
			submitError = err.message || 'Failed to update team';
		} finally {
			isSubmitting = false;
		}
	}

	async function handleDelete() {
		isDeleting = true;
		submitError = null;

		try {
			await apiClient.adminDeleteTeam(teamId);
			goto('/admin/teams');
		} catch (err: any) {
			submitError = err.message || 'Failed to delete team';
			showDeleteConfirm = false;
		} finally {
			isDeleting = false;
		}
	}

	function handleEdit() {
		isEditing = true;
		submitError = null;
	}

	function handleCancelEdit() {
		if (team) {
			// Reset form data
			formData.displayName = team.displayName;
			formData.description = team.description || '';
			formData.ownerUserId = team.ownerUserId || '';
			formData.status = team.status;
		}
		isEditing = false;
		errors = {};
		submitError = null;
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
					<h1 class="text-xl font-bold text-gray-900">Team Details</h1>
				</div>
				{#if team && !isEditing}
					<div class="flex gap-2">
						<button
							onclick={handleEdit}
							class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
						>
							Edit
						</button>
						<button
							onclick={() => (showDeleteConfirm = true)}
							class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700"
						>
							Delete
						</button>
					</div>
				{/if}
			</div>
		</div>
	</nav>

	<main class="max-w-4xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
		<!-- Error Message -->
		{#if error}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{error}</p>
			</div>
		{/if}

		<!-- Submit Error Message -->
		{#if submitError}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{submitError}</p>
			</div>
		{/if}

		{#if isLoading}
			<div class="flex justify-center items-center py-12">
				<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
			</div>
		{:else if team}
			<!-- View Mode -->
			{#if !isEditing}
				<div class="bg-white rounded-lg shadow-md p-6 space-y-6">
					<div class="grid grid-cols-2 gap-6">
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Team Name</label>
							<p class="text-gray-900 font-mono">{team.name}</p>
							<p class="text-xs text-gray-500 mt-1">Immutable identifier</p>
						</div>
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Status</label>
							<span
								class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {getStatusColor(
									team.status
								)}"
							>
								{team.status.charAt(0).toUpperCase() + team.status.slice(1)}
							</span>
						</div>
					</div>

					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Display Name</label>
						<p class="text-gray-900">{team.displayName}</p>
					</div>

					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Description</label>
						<p class="text-gray-900">{team.description || '-'}</p>
					</div>

					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Team Owner</label>
						<p class="text-gray-900">{team.ownerUserId || 'None'}</p>
					</div>

					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Envoy Admin Port</label>
						<p class="text-gray-900 font-mono">{team.envoyAdminPort ?? 'Not allocated'}</p>
						<p class="text-xs text-gray-500 mt-1">Auto-allocated for this team's Envoy instance</p>
					</div>

					<div class="grid grid-cols-2 gap-6 pt-4 border-t border-gray-200">
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Created</label>
							<p class="text-gray-600 text-sm">{formatDate(team.createdAt)}</p>
						</div>
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Last Updated</label>
							<p class="text-gray-600 text-sm">{formatDate(team.updatedAt)}</p>
						</div>
					</div>
				</div>
			{:else}
				<!-- Edit Mode -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<form
						onsubmit={(e) => {
							e.preventDefault();
							handleUpdate();
						}}
					>
						<div class="space-y-6">
							<!-- Team Name (read-only) -->
							<div>
								<label class="block text-sm font-medium text-gray-700 mb-2">Team Name</label>
								<input
									type="text"
									value={team.name}
									disabled
									class="w-full px-3 py-2 border border-gray-300 rounded-md bg-gray-50 text-gray-500 cursor-not-allowed"
								/>
								<p class="mt-1 text-xs text-gray-500">Team name cannot be changed</p>
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
								/>
								{#if errors.displayName}
									<p class="mt-1 text-sm text-red-600">{errors.displayName}</p>
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
										<option value="">No owner</option>
										{#each users as user}
											<option value={user.id}>{user.name} ({user.email})</option>
										{/each}
									</select>
								{/if}
							</div>

							<!-- Status -->
							<div>
								<label for="status" class="block text-sm font-medium text-gray-700 mb-2">
									Status
								</label>
								<select
									id="status"
									bind:value={formData.status}
									class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								>
									<option value="active">Active</option>
									<option value="suspended">Suspended</option>
									<option value="archived">Archived</option>
								</select>
								<p class="mt-1 text-xs text-gray-500">
									Suspended teams cannot modify resources. Archived teams are read-only.
								</p>
							</div>
						</div>

						<!-- Form Actions -->
						<div class="mt-6 flex justify-end gap-3">
							<button
								type="button"
								onclick={handleCancelEdit}
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
								{isSubmitting ? 'Updating...' : 'Update Team'}
							</button>
						</div>
					</form>
				</div>
			{/if}
		{/if}
	</main>
</div>

<!-- Delete Confirmation Modal -->
{#if showDeleteConfirm}
	<div
		class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50"
		onclick={() => (showDeleteConfirm = false)}
	>
		<div
			class="relative top-20 mx-auto p-5 border w-96 shadow-lg rounded-md bg-white"
			onclick={(e) => e.stopPropagation()}
		>
			<div class="mt-3">
				<h3 class="text-lg font-medium leading-6 text-gray-900 mb-2">Delete Team</h3>
				<div class="mt-2 px-7 py-3">
					<p class="text-sm text-gray-500">
						Are you sure you want to delete this team? This action cannot be undone.
					</p>
					<p class="text-sm text-gray-500 mt-2">
						Note: This will fail if there are resources (listeners, routes, clusters, etc.)
						referencing this team.
					</p>
				</div>
				<div class="flex justify-end gap-3 px-4 py-3">
					<button
						onclick={() => (showDeleteConfirm = false)}
						disabled={isDeleting}
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 disabled:opacity-50"
					>
						Cancel
					</button>
					<button
						onclick={handleDelete}
						disabled={isDeleting}
						class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700 disabled:opacity-50"
					>
						{isDeleting ? 'Deleting...' : 'Delete'}
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
