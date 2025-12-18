<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import type { UserWithTeamsResponse, UserTeamMembership, UpdateUserRequest, CreateTeamMembershipRequest } from '$lib/api/types';
	import ScopeSelector from '$lib/components/ScopeSelector.svelte';

	let user = $state<UserWithTeamsResponse | null>(null);
	let availableTeams = $state<string[]>([]);
	let isLoading = $state(true);
	let isLoadingTeams = $state(false);
	let error = $state<string | null>(null);

	// Edit state
	let isEditingName = $state(false);
	let editedName = $state('');

	// Team membership state
	let showAddTeamModal = $state(false);
	let newTeam = $state({ team: '', scopes: [] as string[] });

	// Suspend modal
	let showSuspendModal = $state(false);
	let suspendReason = $state('');

	// Delete modal
	let showDeleteModal = $state(false);

	// Status update
	let isUpdatingStatus = $state(false);

	const userId = $derived($page.params.id);

	onMount(async () => {
		// Check authentication and admin access
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!sessionInfo.isAdmin) {
				goto('/dashboard');
				return;
			}
			await Promise.all([loadUser(), loadTeams()]);
		} catch (err) {
			goto('/login');
		}
	});

	async function loadTeams() {
		isLoadingTeams = true;
		try {
			// Admin users should see all teams from the admin endpoint
			const response = await apiClient.adminListTeams(100, 0);
			availableTeams = response.teams.map(t => t.name);
		} catch (err: any) {
			console.error('Failed to load teams:', err);
			// Non-fatal error
		} finally {
			isLoadingTeams = false;
		}
	}

	async function loadUser() {
		if (!userId) return;

		isLoading = true;
		error = null;

		try {
			user = await apiClient.getUser(userId);
		} catch (err: any) {
			error = err.message || 'Failed to load user';
		} finally {
			isLoading = false;
		}
	}

	function startEditName() {
		if (user) {
			editedName = user.name;
			isEditingName = true;
		}
	}

	async function saveEditName() {
		if (!user || !editedName.trim()) return;

		try {
			const request: UpdateUserRequest = {
				name: editedName
			};
			const updated = await apiClient.updateUser(user.id, request);
			user.name = updated.name;
			isEditingName = false;
		} catch (err: any) {
			error = err.message || 'Failed to update name';
		}
	}

	function cancelEditName() {
		isEditingName = false;
		editedName = '';
	}

	async function toggleAdminRole() {
		if (!user) return;

		try {
			const request: UpdateUserRequest = {
				isAdmin: !user.isAdmin
			};
			const updated = await apiClient.updateUser(user.id, request);
			user.isAdmin = updated.isAdmin;
		} catch (err: any) {
			error = err.message || 'Failed to update role';
		}
	}

	async function toggleStatus(newStatus: 'Active' | 'Suspended') {
		if (!user || isUpdatingStatus) return;

		isUpdatingStatus = true;
		try {
			const request: UpdateUserRequest = {
				status: newStatus
			};
			const updated = await apiClient.updateUser(user.id, request);
			user.status = updated.status;

			if (newStatus === 'Suspended') {
				showSuspendModal = false;
				suspendReason = '';
			}
		} catch (err: any) {
			error = err.message || `Failed to ${newStatus.toLowerCase()} user`;
		} finally {
			isUpdatingStatus = false;
		}
	}

	function handleSuspend() {
		showSuspendModal = true;
	}

	function confirmSuspend() {
		toggleStatus('Suspended');
	}

	function cancelSuspend() {
		showSuspendModal = false;
		suspendReason = '';
	}

	async function handleReactivate() {
		await toggleStatus('Active');
	}

	function openAddTeamModal() {
		newTeam = { team: '', scopes: [] };
		showAddTeamModal = true;
	}

	function toggleScope(scope: string) {
		if (newTeam.scopes.includes(scope)) {
			newTeam.scopes = newTeam.scopes.filter(s => s !== scope);
		} else {
			newTeam.scopes = [...newTeam.scopes, scope];
		}
	}

	async function addTeamMembership() {
		if (!user || !newTeam.team.trim()) return;

		try {
			// Transform global scopes to team-scoped format
			// e.g., "listeners:read" -> "team:engineering:listeners:read"
			const teamScopedScopes = newTeam.scopes.map(scope =>
				`team:${newTeam.team}:${scope}`
			);

			const request: CreateTeamMembershipRequest = {
				userId: user.id,
				team: newTeam.team,
				scopes: teamScopedScopes
			};
			const membership = await apiClient.addTeamMembership(user.id, request);
			user.teams = [...user.teams, membership];
			showAddTeamModal = false;
			newTeam = { team: '', scopes: [] };
		} catch (err: any) {
			error = err.message || 'Failed to add team membership';
		}
	}

	async function removeTeamMembership(team: string) {
		if (!user) return;

		if (!confirm(`Remove user from team "${team}"?`)) {
			return;
		}

		try {
			await apiClient.removeTeamMembership(user.id, team);
			user.teams = user.teams.filter(t => t.team !== team);
		} catch (err: any) {
			error = err.message || 'Failed to remove team membership';
		}
	}

	function handleDelete() {
		showDeleteModal = true;
	}

	async function confirmDelete() {
		if (!user) return;

		try {
			await apiClient.deleteUser(user.id);
			goto('/admin/users');
		} catch (err: any) {
			error = err.message || 'Failed to delete user';
			showDeleteModal = false;
		}
	}

	function cancelDelete() {
		showDeleteModal = false;
	}

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleString('en-US', {
			year: 'numeric',
			month: 'long',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function getStatusColor(status: string): string {
		switch (status.toLowerCase()) {
			case 'active':
				return 'bg-green-100 text-green-800';
			case 'suspended':
				return 'bg-red-100 text-red-800';
			case 'inactive':
				return 'bg-gray-100 text-gray-800';
			default:
				return 'bg-gray-100 text-gray-800';
		}
	}
</script>

<div class="min-h-screen bg-gray-50">
	<!-- Navigation -->
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="w-full px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a
						href="/admin/users"
						class="text-blue-600 hover:text-blue-800"
						aria-label="Back to users"
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
					<h1 class="text-xl font-bold text-gray-900">User Details</h1>
				</div>
				{#if user}
					<div class="flex gap-3">
						{#if user.status === 'Suspended'}
							<button
								onclick={handleReactivate}
								disabled={isUpdatingStatus}
								class="px-4 py-2 text-sm font-medium text-white bg-green-600 rounded-md hover:bg-green-700 disabled:opacity-50"
							>
								Reactivate
							</button>
						{:else}
							<button
								onclick={handleSuspend}
								class="px-4 py-2 text-sm font-medium text-white bg-yellow-600 rounded-md hover:bg-yellow-700"
							>
								Suspend
							</button>
						{/if}
						<button
							onclick={handleDelete}
							class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700"
						>
							Delete
						</button>
					</div>
				{/if}
			</div>
		</div>
	</nav>

	<main class="w-full px-4 sm:px-6 lg:px-8 py-8">
		<!-- Error Message -->
		{#if error}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{error}</p>
			</div>
		{/if}

		{#if isLoading}
			<div class="flex justify-center items-center py-12">
				<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
			</div>
		{:else if user}
			<!-- User Info Section -->
			<div class="bg-white rounded-lg shadow-md p-6 mb-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4">User Information</h2>
				<div class="grid grid-cols-1 md:grid-cols-2 gap-6">
					<!-- Name -->
					<div>
						<label class="block text-sm font-medium text-gray-500 mb-2">Name</label>
						{#if isEditingName}
							<div class="flex gap-2">
								<input
									type="text"
									bind:value={editedName}
									class="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<button
									onclick={saveEditName}
									class="px-3 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
								>
									Save
								</button>
								<button
									onclick={cancelEditName}
									class="px-3 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
								>
									Cancel
								</button>
							</div>
						{:else}
							<div class="flex items-center gap-2">
								<span class="text-gray-900">{user.name}</span>
								<button
									onclick={startEditName}
									class="text-blue-600 hover:text-blue-800 text-sm"
								>
									Edit
								</button>
							</div>
						{/if}
					</div>

					<!-- Email -->
					<div>
						<label class="block text-sm font-medium text-gray-500 mb-2">Email</label>
						<span class="text-gray-900">{user.email}</span>
					</div>

					<!-- Status -->
					<div>
						<label class="block text-sm font-medium text-gray-500 mb-2">Status</label>
						<span
							class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {getStatusColor(
								user.status
							)}"
						>
							{user.status}
						</span>
					</div>

					<!-- Role -->
					<div>
						<label class="block text-sm font-medium text-gray-500 mb-2">Role</label>
						<div class="flex items-center gap-2">
							{#if user.isAdmin}
								<span
									class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-purple-100 text-purple-800"
								>
									Admin
								</span>
							{:else}
								<span
									class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800"
								>
									Developer
								</span>
							{/if}
							<button
								onclick={toggleAdminRole}
								class="text-blue-600 hover:text-blue-800 text-sm"
							>
								{user.isAdmin ? 'Make Developer' : 'Make Admin'}
							</button>
						</div>
					</div>

					<!-- Created At -->
					<div>
						<label class="block text-sm font-medium text-gray-500 mb-2">Created</label>
						<span class="text-gray-900">{formatDate(user.createdAt)}</span>
					</div>

					<!-- Updated At -->
					<div>
						<label class="block text-sm font-medium text-gray-500 mb-2">Last Updated</label>
						<span class="text-gray-900">{formatDate(user.updatedAt)}</span>
					</div>
				</div>
			</div>

			<!-- Team Memberships Section -->
			<div class="bg-white rounded-lg shadow-md p-6 mb-6">
				<div class="flex justify-between items-center mb-4">
					<h2 class="text-lg font-semibold text-gray-900">Team Memberships</h2>
					<button
						onclick={openAddTeamModal}
						class="px-3 py-2 text-sm font-medium text-blue-600 hover:text-blue-800 border border-blue-300 rounded-md hover:bg-blue-50"
					>
						Add Team
					</button>
				</div>

				{#if user.teams.length === 0}
					<p class="text-gray-500 text-sm">No team memberships</p>
				{:else}
					<div class="space-y-3">
						{#each user.teams as membership (membership.id)}
							<div class="flex items-start justify-between p-4 border border-gray-200 rounded-md">
								<div>
									<h3 class="font-medium text-gray-900">{membership.team}</h3>
									<div class="mt-2 flex flex-wrap gap-1">
										{#each membership.scopes as scope}
											{@const displayScope = scope.startsWith(`team:${membership.team}:`)
												? scope.substring(`team:${membership.team}:`.length)
												: scope}
											<span
												class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-gray-100 text-gray-800"
											>
												{displayScope}
											</span>
										{/each}
									</div>
									<p class="mt-2 text-xs text-gray-500">
										Added {formatDate(membership.createdAt)}
									</p>
								</div>
								<button
									onclick={() => removeTeamMembership(membership.team)}
									class="text-red-600 hover:text-red-800 text-sm"
								>
									Remove
								</button>
							</div>
						{/each}
					</div>
				{/if}
			</div>

			<!-- Effective Scopes Section (calculated from all teams) -->
			<div class="bg-white rounded-lg shadow-md p-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Effective Scopes</h2>
				{#if user.isAdmin}
					<div class="bg-purple-50 border-l-4 border-purple-500 p-4">
						<p class="text-purple-800 text-sm">
							This user has administrator privileges with access to all scopes.
						</p>
					</div>
				{:else}
					{#if user.teams.length === 0}
						<p class="text-gray-500 text-sm">No scopes (no team memberships)</p>
					{:else}
						{@const allScopes = [...new Set(user.teams.flatMap(t => t.scopes))]}
						<div class="flex flex-wrap gap-2">
							{#each allScopes as scope}
								{@const displayScope = scope.includes(':') && scope.split(':').length === 4
									? scope.split(':').slice(2).join(':')
									: scope}
								<span
									class="inline-flex items-center px-3 py-1 rounded-full text-sm font-medium bg-blue-100 text-blue-800"
								>
									{displayScope}
								</span>
							{/each}
						</div>
					{/if}
				{/if}
			</div>
		{/if}
	</main>
</div>

<!-- Add Team Modal -->
{#if showAddTeamModal && user}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
	>
		<div class="bg-white rounded-lg shadow-xl p-6 max-w-md w-full">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Add Team Membership</h2>

			<div class="space-y-4">
				<!-- Team Selection -->
				<div>
					<label for="teamName" class="block text-sm font-medium text-gray-700 mb-2">
						Team <span class="text-red-500">*</span>
					</label>
					{#if isLoadingTeams}
						<div class="text-sm text-gray-500">Loading teams...</div>
					{:else if availableTeams.length === 0}
						<div class="text-sm text-gray-500">No teams available</div>
					{:else}
						<select
							id="teamName"
							bind:value={newTeam.team}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						>
							<option value="">Select a team</option>
							{#each availableTeams as team}
								{#if !user?.teams.some(m => m.team === team)}
									<option value={team}>{team}</option>
								{/if}
							{/each}
						</select>
					{/if}
					<p class="mt-1 text-xs text-gray-500">
						Only showing teams the user is not already a member of
					</p>
				</div>

				<!-- Scopes -->
				<ScopeSelector
					bind:selectedScopes={newTeam.scopes}
					onScopeToggle={toggleScope}
					required={false}
				/>
			</div>

			<div class="mt-6 flex justify-end gap-3">
				<button
					onclick={() => showAddTeamModal = false}
					class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200"
				>
					Cancel
				</button>
				<button
					onclick={addTeamMembership}
					disabled={!newTeam.team.trim()}
					class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50"
				>
					Add Team
				</button>
			</div>
		</div>
	</div>
{/if}

<!-- Suspend Modal -->
{#if showSuspendModal && user}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
	>
		<div class="bg-white rounded-lg shadow-xl p-6 max-w-md w-full">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Suspend User</h2>
			<p class="text-sm text-gray-600 mb-4">
				Are you sure you want to suspend <strong>{user.name}</strong>? This will:
			</p>
			<ul class="list-disc list-inside text-sm text-gray-600 mb-6 space-y-1">
				<li>Revoke all active sessions</li>
				<li>Revoke all personal access tokens</li>
				<li>Prevent the user from logging in</li>
			</ul>

			<div class="mb-4">
				<label for="suspendReason" class="block text-sm font-medium text-gray-700 mb-2">
					Reason (optional)
				</label>
				<textarea
					id="suspendReason"
					bind:value={suspendReason}
					rows="3"
					placeholder="Enter reason for suspension..."
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				></textarea>
			</div>

			<div class="flex justify-end gap-3">
				<button
					onclick={cancelSuspend}
					disabled={isUpdatingStatus}
					class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200 disabled:opacity-50"
				>
					Cancel
				</button>
				<button
					onclick={confirmSuspend}
					disabled={isUpdatingStatus}
					class="px-4 py-2 text-sm font-medium text-white bg-yellow-600 rounded-md hover:bg-yellow-700 disabled:opacity-50"
				>
					{isUpdatingStatus ? 'Suspending...' : 'Suspend User'}
				</button>
			</div>
		</div>
	</div>
{/if}

<!-- Delete Confirmation Modal -->
{#if showDeleteModal && user}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
	>
		<div class="bg-white rounded-lg shadow-xl p-6 max-w-md w-full">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Delete User</h2>
			<p class="text-sm text-gray-600 mb-6">
				Are you sure you want to delete <strong>{user.name}</strong>? This action cannot be
				undone and will permanently remove:
			</p>
			<ul class="list-disc list-inside text-sm text-gray-600 mb-6 space-y-1">
				<li>User account and credentials</li>
				<li>All team memberships</li>
				<li>All personal access tokens</li>
				<li>All active sessions</li>
			</ul>
			<div class="flex justify-end gap-3">
				<button
					onclick={cancelDelete}
					class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200"
				>
					Cancel
				</button>
				<button
					onclick={confirmDelete}
					class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700"
				>
					Delete User
				</button>
			</div>
		</div>
	</div>
{/if}
