<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import type {
		OrganizationResponse,
		UpdateOrganizationRequest,
		OrgMembershipResponse,
		OrgStatus,
		OrgRole,
		UserResponse
	} from '$lib/api/types';
	import { isOrgAdmin } from '$lib/stores/org';
	import OrgInvitationsSection from '$lib/components/OrgInvitationsSection.svelte';

	let orgId = $derived($page.params.id ?? '');

	let org = $state<OrganizationResponse | null>(null);
	let members = $state<OrgMembershipResponse[]>([]);
	let users = $state<UserResponse[]>([]);
	let userScopes = $state<string[]>([]);
	let isPlatformAdmin = $state(false);
	let isLoading = $state(true);
	let isLoadingMembers = $state(true);
	let isLoadingUsers = $state(true);
	let isEditing = $state(false);
	let isSubmitting = $state(false);
	let isDeleting = $state(false);
	let error = $state<string | null>(null);
	let submitError = $state<string | null>(null);
	let memberError = $state<string | null>(null);
	let showDeleteConfirm = $state(false);
	let showAddMember = $state(false);
	let showRemoveMemberConfirm = $state<string | null>(null);

	let formData = $state({
		displayName: '',
		description: '',
		status: 'active' as OrgStatus
	});

	let addMemberData = $state({
		userId: '',
		role: 'member' as OrgRole
	});

	let errors = $state<Record<string, string>>({});

	onMount(async () => {
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			userScopes = sessionInfo.scopes;
			isPlatformAdmin = sessionInfo.isAdmin;
			if (!sessionInfo.isAdmin && !isOrgAdmin(sessionInfo.scopes)) {
				goto('/dashboard');
				return;
			}

			const promises: Promise<void>[] = [loadOrg(), loadMembers()];
			if (isPlatformAdmin) {
				promises.push(loadUsers());
			}
			await Promise.all(promises);
		} catch {
			goto('/login');
		}
	});

	async function loadOrg() {
		isLoading = true;
		error = null;

		try {
			org = await apiClient.getOrganization(orgId);
			formData.displayName = org.displayName;
			formData.description = org.description || '';
			formData.status = org.status;
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to load organization';
		} finally {
			isLoading = false;
		}
	}

	async function loadMembers() {
		isLoadingMembers = true;
		memberError = null;

		try {
			members = await apiClient.listOrgMembers(orgId);
		} catch (err: unknown) {
			memberError = err instanceof Error ? err.message : 'Failed to load members';
		} finally {
			isLoadingMembers = false;
		}
	}

	async function loadUsers() {
		isLoadingUsers = true;
		try {
			const response = await apiClient.listUsers(100, 0);
			users = response.items;
		} catch {
			// Non-fatal
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

	function getStatusColor(status: OrgStatus): string {
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

	function getRoleColor(role: OrgRole): string {
		switch (role) {
			case 'owner':
				return 'bg-purple-100 text-purple-800';
			case 'admin':
				return 'bg-blue-100 text-blue-800';
			case 'member':
				return 'bg-gray-100 text-gray-800';
			case 'viewer':
				return 'bg-gray-50 text-gray-600';
			default:
				return 'bg-gray-100 text-gray-800';
		}
	}

	const orgRoles: OrgRole[] = ['owner', 'admin', 'member', 'viewer'];

	function validateForm(): boolean {
		const newErrors: Record<string, string> = {};

		if (!formData.displayName.trim()) {
			newErrors.displayName = 'Display name is required';
		} else if (formData.displayName.length > 255) {
			newErrors.displayName = 'Display name must be 255 characters or less';
		}

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
			const request: UpdateOrganizationRequest = {
				displayName: formData.displayName,
				description: formData.description || undefined,
				status: formData.status
			};

			const updatedOrg = await apiClient.updateOrganization(orgId, request);
			org = updatedOrg;
			isEditing = false;
		} catch (err: unknown) {
			submitError = err instanceof Error ? err.message : 'Failed to update organization';
		} finally {
			isSubmitting = false;
		}
	}

	async function handleDelete() {
		isDeleting = true;
		submitError = null;

		try {
			await apiClient.deleteOrganization(orgId);
			goto('/admin/organizations');
		} catch (err: unknown) {
			submitError = err instanceof Error ? err.message : 'Failed to delete organization';
			showDeleteConfirm = false;
		} finally {
			isDeleting = false;
		}
	}

	async function handleAddMember() {
		if (!addMemberData.userId) {
			memberError = 'Please select a user';
			return;
		}

		memberError = null;

		try {
			await apiClient.addOrgMember(orgId, {
				userId: addMemberData.userId,
				role: addMemberData.role
			});
			showAddMember = false;
			addMemberData = { userId: '', role: 'member' };
			await loadMembers();
		} catch (err: unknown) {
			memberError = err instanceof Error ? err.message : 'Failed to add member';
		}
	}

	async function handleChangeRole(userId: string, newRole: OrgRole) {
		memberError = null;

		try {
			await apiClient.updateOrgMemberRole(orgId, userId, newRole);
			await loadMembers();
		} catch (err: unknown) {
			memberError = err instanceof Error ? err.message : 'Failed to update member role';
		}
	}

	async function handleRemoveMember(userId: string) {
		memberError = null;

		try {
			await apiClient.removeOrgMember(orgId, userId);
			showRemoveMemberConfirm = null;
			await loadMembers();
		} catch (err: unknown) {
			memberError = err instanceof Error ? err.message : 'Failed to remove member';
		}
	}

	function handleEdit() {
		isEditing = true;
		submitError = null;
	}

	function handleCancelEdit() {
		if (org) {
			formData.displayName = org.displayName;
			formData.description = org.description || '';
			formData.status = org.status;
		}
		isEditing = false;
		errors = {};
		submitError = null;
	}

	// Filter users not already members
	let availableUsers = $derived.by(() => {
		const memberUserIds = new Set(members.map((m) => m.userId));
		return users.filter((u) => !memberUserIds.has(u.id));
	});
</script>

<div class="min-h-screen bg-gray-50">
	<!-- Navigation -->
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="w-full px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a
						href="/admin/organizations"
						class="text-blue-600 hover:text-blue-800"
						aria-label="Back to organizations"
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
					<h1 class="text-xl font-bold text-gray-900">Organization Details</h1>
				</div>
				{#if org && !isEditing}
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

	<main class="w-full px-4 sm:px-6 lg:px-8 py-8">
		<!-- Error Messages -->
		{#if error}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{error}</p>
			</div>
		{/if}

		{#if submitError}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{submitError}</p>
			</div>
		{/if}

		{#if isLoading}
			<div class="flex justify-center items-center py-12">
				<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
			</div>
		{:else if org}
			<!-- View Mode -->
			{#if !isEditing}
				<div class="bg-white rounded-lg shadow-md p-6 space-y-6">
					<div class="grid grid-cols-2 gap-6">
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Organization Name</label>
							<p class="text-gray-900 font-mono">{org.name}</p>
							<p class="text-xs text-gray-500 mt-1">Immutable identifier</p>
						</div>
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Status</label>
							<span
								class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {getStatusColor(
									org.status
								)}"
							>
								{org.status.charAt(0).toUpperCase() + org.status.slice(1)}
							</span>
						</div>
					</div>

					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Display Name</label>
						<p class="text-gray-900">{org.displayName}</p>
					</div>

					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">Description</label>
						<p class="text-gray-900">{org.description || '-'}</p>
					</div>

					<div class="grid grid-cols-2 gap-6 pt-4 border-t border-gray-200">
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Created</label>
							<p class="text-gray-600 text-sm">{formatDate(org.createdAt)}</p>
						</div>
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Last Updated</label>
							<p class="text-gray-600 text-sm">{formatDate(org.updatedAt)}</p>
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
							<!-- Organization Name (read-only) -->
							<div>
								<label class="block text-sm font-medium text-gray-700 mb-2">Organization Name</label>
								<input
									type="text"
									value={org.name}
									disabled
									class="w-full px-3 py-2 border border-gray-300 rounded-md bg-gray-50 text-gray-500 cursor-not-allowed"
								/>
								<p class="mt-1 text-xs text-gray-500">Organization name cannot be changed</p>
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
									Suspended organizations cannot modify resources. Archived organizations are read-only.
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
								{isSubmitting ? 'Updating...' : 'Update Organization'}
							</button>
						</div>
					</form>
				</div>
			{/if}

			<!-- Members Section -->
			<div class="mt-8">
				<div class="flex items-center justify-between mb-4">
					<h2 class="text-lg font-semibold text-gray-900">Members</h2>
					{#if isPlatformAdmin}
						<button
							onclick={() => (showAddMember = true)}
							class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
						>
							Add Member
						</button>
					{/if}
				</div>

				{#if memberError}
					<div class="mb-4 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
						<p class="text-red-800 text-sm">{memberError}</p>
					</div>
				{/if}

				<!-- Add Member Form (platform admins only) -->
				{#if isPlatformAdmin && showAddMember}
					<div class="bg-white rounded-lg shadow-md p-6 mb-4">
						<h3 class="text-sm font-medium text-gray-900 mb-4">Add New Member</h3>
						<div class="grid grid-cols-1 md:grid-cols-3 gap-4">
							<div class="md:col-span-2">
								<label for="addMemberUser" class="block text-sm font-medium text-gray-700 mb-1">
									User
								</label>
								{#if isLoadingUsers}
									<div class="text-sm text-gray-500">Loading users...</div>
								{:else}
									<select
										id="addMemberUser"
										bind:value={addMemberData.userId}
										class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									>
										<option value="">Select a user...</option>
										{#each availableUsers as user}
											<option value={user.id}>{user.name} ({user.email})</option>
										{/each}
									</select>
								{/if}
							</div>
							<div>
								<label for="addMemberRole" class="block text-sm font-medium text-gray-700 mb-1">
									Role
								</label>
								<select
									id="addMemberRole"
									bind:value={addMemberData.role}
									class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								>
									{#each orgRoles as role}
										<option value={role}>{role.charAt(0).toUpperCase() + role.slice(1)}</option>
									{/each}
								</select>
							</div>
						</div>
						<div class="mt-4 flex justify-end gap-3">
							<button
								onclick={() => {
									showAddMember = false;
									addMemberData = { userId: '', role: 'member' };
									memberError = null;
								}}
								class="px-3 py-1.5 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
							>
								Cancel
							</button>
							<button
								onclick={handleAddMember}
								class="px-3 py-1.5 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700"
							>
								Add
							</button>
						</div>
					</div>
				{/if}

				<!-- Members Table -->
				<div class="bg-white rounded-lg shadow-md overflow-hidden">
					{#if isLoadingMembers}
						<div class="flex justify-center items-center py-8">
							<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
						</div>
					{:else if members.length === 0}
						<div class="text-center py-8">
							<p class="text-gray-500">No members yet</p>
						</div>
					{:else}
						<div class="overflow-x-auto">
							<table class="min-w-full divide-y divide-gray-200">
								<thead class="bg-gray-50">
									<tr>
										<th
											class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
										>
											User
										</th>
										<th
											class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
										>
											Role
										</th>
										<th
											class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
										>
											Added
										</th>
										<th
											class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
										>
											Actions
										</th>
									</tr>
								</thead>
								<tbody class="bg-white divide-y divide-gray-200">
									{#each members as member (member.id)}
										<tr class="hover:bg-gray-50">
											<td class="px-6 py-4 whitespace-nowrap">
												<div class="text-sm font-medium text-gray-900">
													{member.userName || 'Unknown'}
												</div>
												<div class="text-xs text-gray-500">
													{member.userEmail || member.userId}
												</div>
											</td>
											<td class="px-6 py-4 whitespace-nowrap">
												<select
													value={member.role}
													onchange={(e) =>
														handleChangeRole(member.userId, e.currentTarget.value as OrgRole)}
													class="text-xs font-medium px-2 py-1 rounded-full border-0 cursor-pointer {getRoleColor(
														member.role
													)}"
												>
													{#each orgRoles as role}
														<option value={role}
															>{role.charAt(0).toUpperCase() + role.slice(1)}</option
														>
													{/each}
												</select>
											</td>
											<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
												{formatDate(member.createdAt)}
											</td>
											<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
												<button
													onclick={() => (showRemoveMemberConfirm = member.userId)}
													class="text-red-600 hover:text-red-900"
												>
													Remove
												</button>
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{/if}
				</div>
			</div>

			<!-- Invitations Section -->
			{#if org}
				<OrgInvitationsSection orgName={org.name} {orgId} {userScopes} />
			{/if}
		{/if}
	</main>
</div>

<!-- Delete Confirmation Modal -->
{#if showDeleteConfirm}
	<div
		class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50"
		onclick={() => (showDeleteConfirm = false)}
		role="dialog"
		aria-modal="true"
		aria-label="Confirm delete organization"
	>
		<div
			class="relative top-20 mx-auto p-5 border w-96 shadow-lg rounded-md bg-white"
			onclick={(e) => e.stopPropagation()}
		>
			<div class="mt-3">
				<h3 class="text-lg font-medium leading-6 text-gray-900 mb-2">Delete Organization</h3>
				<div class="mt-2 px-7 py-3">
					<p class="text-sm text-gray-500">
						Are you sure you want to delete this organization? This action cannot be undone.
					</p>
					<p class="text-sm text-gray-500 mt-2">
						Note: This will fail if there are teams or resources belonging to this organization.
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

<!-- Remove Member Confirmation Modal -->
{#if showRemoveMemberConfirm}
	<div
		class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50"
		onclick={() => (showRemoveMemberConfirm = null)}
		role="dialog"
		aria-modal="true"
		aria-label="Confirm remove member"
	>
		<div
			class="relative top-20 mx-auto p-5 border w-96 shadow-lg rounded-md bg-white"
			onclick={(e) => e.stopPropagation()}
		>
			<div class="mt-3">
				<h3 class="text-lg font-medium leading-6 text-gray-900 mb-2">Remove Member</h3>
				<div class="mt-2 px-7 py-3">
					<p class="text-sm text-gray-500">
						Are you sure you want to remove this member from the organization?
					</p>
				</div>
				<div class="flex justify-end gap-3 px-4 py-3">
					<button
						onclick={() => (showRemoveMemberConfirm = null)}
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
					>
						Cancel
					</button>
					<button
						onclick={() => {
							if (showRemoveMemberConfirm) handleRemoveMember(showRemoveMemberConfirm);
						}}
						class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700"
					>
						Remove
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
