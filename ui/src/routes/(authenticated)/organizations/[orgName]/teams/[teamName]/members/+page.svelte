<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import type {
		OrgTeamMemberResponse,
		OrgMembershipResponse,
		AddOrgTeamMemberRequest,
		GrantResponse
	} from '$lib/api/types';
	import { isOrgAdmin } from '$lib/stores/org';
	import PermissionMatrix from '$lib/components/PermissionMatrix.svelte';

	let orgName = $derived($page.params.orgName ?? '');
	let teamName = $derived($page.params.teamName ?? '');

	let members = $state<OrgTeamMemberResponse[]>([]);
	let orgMembers = $state<OrgMembershipResponse[]>([]);
	let teamId = $state('');
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);

	// Add member state
	let selectedUserId = $state('');
	let isAdding = $state(false);

	// Edit permissions state
	let expandedMemberId = $state<string | null>(null);
	let memberGrants = $state<Record<string, GrantResponse[]>>({});
	let isLoadingGrants = $state<Record<string, boolean>>({});

	// Remove member state
	let showRemoveConfirm = $state<string | null>(null);
	let isRemoving = $state(false);

	onMount(async () => {
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!isOrgAdmin(sessionInfo.orgRole)) {
				goto(`/organizations/${orgName}/teams/${teamName}`);
				return;
			}
			const orgId = sessionInfo.orgId;
			if (!orgId) {
				error = 'No organization context found';
				isLoading = false;
				return;
			}
			await Promise.all([loadTeam(), loadMembers(), loadOrgMembers(orgId)]);
		} catch {
			goto('/login');
		}
	});

	async function loadTeam() {
		try {
			const team = await apiClient.getOrgTeam(orgName, teamName);
			teamId = team.id;
		} catch {
			// Non-fatal — PermissionMatrix will be unavailable
		}
	}

	async function loadMembers() {
		isLoading = true;
		error = null;
		try {
			const response = await apiClient.listTeamMembers(orgName, teamName);
			members = response.members;
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to load team members';
		} finally {
			isLoading = false;
		}
	}

	async function loadOrgMembers(orgId: string) {
		try {
			orgMembers = await apiClient.listOrgMembers(orgId);
		} catch {
			// Non-fatal: we just can't show the add member dropdown
		}
	}

	// Org members not already on the team
	let availableOrgMembers = $derived.by(() => {
		const memberIds = new Set(members.map((m) => m.userId));
		return orgMembers.filter((m) => !memberIds.has(m.userId));
	});

	// Display info for a team member, with org member fallback
	function getMemberDisplay(member: OrgTeamMemberResponse): string {
		if (member.userName) return member.userName;
		if (member.userEmail) return member.userEmail;
		const orgMember = orgMembers.find((m) => m.userId === member.userId);
		if (orgMember) {
			return orgMember.userName || orgMember.userEmail || member.userId;
		}
		return member.userId;
	}

	function getMemberEmail(member: OrgTeamMemberResponse): string | null {
		if (member.userEmail) return member.userEmail;
		const orgMember = orgMembers.find((m) => m.userId === member.userId);
		return orgMember?.userEmail ?? null;
	}

	async function handleAddMember() {
		if (!selectedUserId) return;

		isAdding = true;
		actionError = null;

		try {
			const request: AddOrgTeamMemberRequest = {
				userId: selectedUserId,
				scopes: []
			};

			const newMember = await apiClient.addTeamMember(orgName, teamName, request);
			members = [...members, newMember];
			selectedUserId = '';
		} catch (err: unknown) {
			actionError = err instanceof Error ? err.message : 'Failed to add member';
		} finally {
			isAdding = false;
		}
	}

	async function toggleMemberPermissions(member: OrgTeamMemberResponse) {
		if (expandedMemberId === member.userId) {
			expandedMemberId = null;
			return;
		}
		expandedMemberId = member.userId;
		if (!memberGrants[member.userId]) {
			await loadMemberGrants(member.userId);
		}
	}

	async function loadMemberGrants(userId: string) {
		isLoadingGrants = { ...isLoadingGrants, [userId]: true };
		try {
			const resp = await apiClient.listPrincipalGrants(orgName, userId);
			memberGrants = { ...memberGrants, [userId]: resp.grants };
		} catch {
			memberGrants = { ...memberGrants, [userId]: [] };
		} finally {
			isLoadingGrants = { ...isLoadingGrants, [userId]: false };
		}
	}

	async function handleRemoveMember(userId: string) {
		isRemoving = true;
		actionError = null;

		try {
			await apiClient.removeTeamMember(orgName, teamName, userId);
			members = members.filter((m) => m.userId !== userId);
			showRemoveConfirm = null;
		} catch (err: unknown) {
			actionError = err instanceof Error ? err.message : 'Failed to remove member';
		} finally {
			isRemoving = false;
		}
	}

	function formatDate(dateString: string): string {
		return new Date(dateString).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}
</script>

<div class="min-h-screen bg-gray-50">
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="w-full px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a
						href="/organizations/{orgName}/teams/{teamName}"
						class="text-blue-600 hover:text-blue-800"
						aria-label="Back to team"
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
					<h1 class="text-xl font-bold text-gray-900">Members — {teamName}</h1>
				</div>
			</div>
		</div>
	</nav>

	<main class="w-full px-4 sm:px-6 lg:px-8 py-8">
		{#if error}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{error}</p>
			</div>
		{/if}

		{#if actionError}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{actionError}</p>
			</div>
		{/if}

		<!-- Add Member Section -->
		{#if availableOrgMembers.length > 0}
			<div class="bg-white rounded-lg shadow-md p-6 mb-6">
				<h2 class="text-lg font-semibold text-gray-900 mb-4">Add Member</h2>
				<div class="space-y-4">
					<div>
						<label for="userSelect" class="block text-sm font-medium text-gray-700 mb-2">
							Select org member to add
						</label>
						<select
							id="userSelect"
							bind:value={selectedUserId}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						>
							<option value="">— Select a member —</option>
							{#each availableOrgMembers as m}
								<option value={m.userId}>
									{m.userName || m.userEmail || m.userId}
									{m.userEmail ? `(${m.userEmail})` : ''}
								</option>
							{/each}
						</select>
					</div>

					{#if selectedUserId}
						<div class="flex justify-end">
							<button
								onclick={handleAddMember}
								disabled={isAdding}
								class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50"
							>
								{isAdding ? 'Adding...' : 'Add Member'}
							</button>
						</div>
					{/if}
				</div>
			</div>
		{/if}

		<!-- Members List -->
		<div class="bg-white rounded-lg shadow-md overflow-hidden">
			<div class="px-6 py-4 border-b border-gray-200">
				<h2 class="text-lg font-semibold text-gray-900">Current Members</h2>
			</div>

			{#if isLoading}
				<div class="flex justify-center items-center py-12">
					<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
				</div>
			{:else if members.length === 0}
				<div class="text-center py-12">
					<p class="text-gray-500">No members yet</p>
				</div>
			{:else}
				<div class="divide-y divide-gray-200">
					{#each members as member (member.userId)}
						<div class="p-6">
							<div class="flex items-start justify-between">
								<div class="flex-1">
									<div class="text-sm font-medium text-gray-900">
										{getMemberDisplay(member)}
									</div>
									{#if getMemberEmail(member)}
										<div class="text-xs text-gray-500">{getMemberEmail(member)}</div>
									{/if}
									<div class="text-xs text-gray-400 mt-1">Added {formatDate(member.createdAt)}</div>
								</div>
								<div class="flex gap-2 ml-4">
									<button
										onclick={() => toggleMemberPermissions(member)}
										class="text-sm text-blue-600 hover:text-blue-900"
									>
										{expandedMemberId === member.userId ? 'Hide Permissions' : 'Edit Permissions'}
									</button>
									<button
										onclick={() => (showRemoveConfirm = member.userId)}
										class="text-sm text-red-600 hover:text-red-900"
									>
										Remove
									</button>
								</div>
							</div>

							{#if expandedMemberId === member.userId}
								<div class="mt-4 border border-gray-200 rounded-md overflow-hidden">
									{#if isLoadingGrants[member.userId]}
										<div class="flex justify-center items-center py-8">
											<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
										</div>
									{:else if teamId}
										<PermissionMatrix
											principalId={member.userId}
											{orgName}
											{teamId}
											{teamName}
											existingGrants={memberGrants[member.userId] ?? []}
											onGrantCreated={() => loadMemberGrants(member.userId)}
											onGrantDeleted={() => loadMemberGrants(member.userId)}
										/>
									{:else}
										<p class="px-4 py-3 text-sm text-gray-500">
											Team ID not available — cannot manage permissions.
										</p>
									{/if}
								</div>
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		</div>
	</main>
</div>

<!-- Remove Member Confirmation Modal -->
{#if showRemoveConfirm}
	<div
		class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50"
		onclick={() => (showRemoveConfirm = null)}
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
						Are you sure you want to remove <strong
							>{(() => {
								const m = members.find((m) => m.userId === showRemoveConfirm);
								return m ? getMemberDisplay(m) : showRemoveConfirm;
							})()}</strong
						> from this team?
					</p>
				</div>
				<div class="flex justify-end gap-3 px-4 py-3">
					<button
						onclick={() => (showRemoveConfirm = null)}
						disabled={isRemoving}
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 disabled:opacity-50"
					>
						Cancel
					</button>
					<button
						onclick={() => {
							if (showRemoveConfirm) handleRemoveMember(showRemoveConfirm);
						}}
						disabled={isRemoving}
						class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700 disabled:opacity-50"
					>
						{isRemoving ? 'Removing...' : 'Remove'}
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
