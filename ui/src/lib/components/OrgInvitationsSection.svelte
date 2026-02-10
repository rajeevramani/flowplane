<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { createInvitationSchema } from '$lib/schemas/auth';
	import { ZodError } from 'zod';
	import { isSystemAdmin } from '$lib/stores/org';
	import type {
		InvitationResponse,
		CreateInvitationResponse,
		InvitableRole,
		InvitationStatus
	} from '$lib/api/types';
	import Badge from './Badge.svelte';
	import Pagination from './Pagination.svelte';
	import DeleteConfirmModal from './DeleteConfirmModal.svelte';

	interface Props {
		orgName: string;
		orgId: string;
		userScopes: string[];
	}

	let { orgName, orgId, userScopes }: Props = $props();

	// State
	let invitations = $state<InvitationResponse[]>([]);
	let total = $state(0);
	let currentPage = $state(1);
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	const pageSize = 25;

	// Invite form state
	let inviteEmail = $state('');
	let inviteRole = $state<InvitableRole>('member');
	let isCreating = $state(false);
	let createError = $state<string | null>(null);
	let fieldErrors = $state<Record<string, string>>({});

	// Success state (after creating invitation)
	let createdInvite = $state<CreateInvitationResponse | null>(null);
	let copyButtonText = $state('Copy Link');
	let copyFallback = $state(false);

	// Revoke modal state
	let revokeTarget = $state<InvitationResponse | null>(null);
	let isRevoking = $state(false);

	// Determine available roles based on user permissions
	const isAdmin = $derived(isSystemAdmin(userScopes));
	const availableRoles = $derived<InvitableRole[]>(
		isAdmin ? ['admin', 'member', 'viewer'] : ['member', 'viewer']
	);

	function getStatusVariant(
		status: InvitationStatus
	): 'yellow' | 'green' | 'gray' | 'red' {
		switch (status) {
			case 'pending':
				return 'yellow';
			case 'accepted':
				return 'green';
			case 'expired':
				return 'gray';
			case 'revoked':
				return 'red';
			default:
				return 'gray';
		}
	}

	function getRoleVariant(role: InvitableRole): 'blue' | 'gray' | 'purple' {
		switch (role) {
			case 'admin':
				return 'blue';
			case 'member':
				return 'gray';
			case 'viewer':
				return 'purple';
			default:
				return 'gray';
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

	function formatRelativeExpiry(dateString: string): string {
		const expiry = new Date(dateString);
		const now = new Date();
		if (expiry <= now) return 'Expired';
		const hoursLeft = Math.round((expiry.getTime() - now.getTime()) / (1000 * 60 * 60));
		if (hoursLeft < 1) return 'Less than 1 hour';
		if (hoursLeft < 24) return `${hoursLeft}h remaining`;
		const daysLeft = Math.round(hoursLeft / 24);
		return `${daysLeft}d remaining`;
	}

	async function loadInvitations() {
		isLoading = true;
		error = null;
		try {
			const offset = (currentPage - 1) * pageSize;
			const response = await apiClient.listOrgInvitations(orgName, pageSize, offset);
			invitations = response.invitations;
			total = response.total;
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to load invitations';
		} finally {
			isLoading = false;
		}
	}

	async function handleCreateInvitation(event: Event) {
		event.preventDefault();
		createError = null;
		fieldErrors = {};
		isCreating = true;

		try {
			createInvitationSchema.parse({ email: inviteEmail, role: inviteRole });

			const response = await apiClient.createOrgInvitation(orgName, {
				email: inviteEmail,
				role: inviteRole
			});

			createdInvite = response;
			inviteEmail = '';
			inviteRole = 'member';

			// Reload the list
			await loadInvitations();
		} catch (err: unknown) {
			if (err instanceof ZodError) {
				const errors: Record<string, string> = {};
				err.issues.forEach((issue) => {
					const field = issue.path[0];
					if (typeof field === 'string') {
						errors[field] = issue.message;
					}
				});
				fieldErrors = errors;
			} else if (err instanceof Error) {
				createError = err.message;
			} else {
				createError = 'Failed to create invitation';
			}
		} finally {
			isCreating = false;
		}
	}

	async function handleCopyLink() {
		if (!createdInvite) return;
		try {
			await navigator.clipboard.writeText(createdInvite.inviteUrl);
			copyButtonText = 'Copied!';
			setTimeout(() => {
				copyButtonText = 'Copy Link';
			}, 2000);
		} catch {
			// Clipboard API not available (non-HTTPS, iframe, etc.) - show fallback
			copyFallback = true;
		}
	}

	function dismissSuccess() {
		createdInvite = null;
		copyFallback = false;
		copyButtonText = 'Copy Link';
	}

	async function handleRevoke() {
		if (!revokeTarget) return;
		isRevoking = true;
		try {
			await apiClient.revokeOrgInvitation(orgName, revokeTarget.id);
			revokeTarget = null;
			await loadInvitations();
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to revoke invitation';
			revokeTarget = null;
		} finally {
			isRevoking = false;
		}
	}

	function handleReInvite(invitation: InvitationResponse) {
		inviteEmail = invitation.email;
		inviteRole = invitation.role;
		createdInvite = null;
	}

	function handlePageChange(page: number) {
		currentPage = page;
		loadInvitations();
	}

	// Initial load
	$effect(() => {
		loadInvitations();
	});

	let totalPages = $derived(Math.ceil(total / pageSize));
</script>

<div class="mt-8">
	<div class="flex items-center justify-between mb-4">
		<h2 class="text-lg font-semibold text-gray-900">Invitations</h2>
	</div>

	{#if error}
		<div class="mb-4 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
			<p class="text-red-800 text-sm">{error}</p>
		</div>
	{/if}

	<!-- Success banner after creating invitation -->
	{#if createdInvite}
		<div class="mb-4 bg-green-50 border border-green-200 rounded-md p-4">
			<div class="flex items-start justify-between">
				<div class="flex-1">
					<p class="text-sm font-medium text-green-800">
						Invitation sent to {createdInvite.email}
					</p>
					<p class="text-xs text-green-700 mt-1">
						Share this link with the user. It can only be viewed once.
					</p>
					{#if copyFallback}
						<input
							type="text"
							readonly
							value={createdInvite.inviteUrl}
							class="mt-2 w-full px-2 py-1 text-xs font-mono bg-white border border-green-300 rounded select-all"
							onclick={(e) => (e.currentTarget as HTMLInputElement).select()}
						/>
					{/if}
				</div>
				<div class="flex items-center gap-2 ml-4">
					<button
						onclick={handleCopyLink}
						class="px-3 py-1.5 text-sm font-medium text-green-700 bg-green-100 rounded-md hover:bg-green-200"
					>
						{copyButtonText}
					</button>
					<button
						onclick={dismissSuccess}
						class="p-1 text-green-600 hover:text-green-800"
						aria-label="Dismiss"
					>
						<svg class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M6 18L18 6M6 6l12 12"
							/>
						</svg>
					</button>
				</div>
			</div>
		</div>
	{/if}

	<!-- Invite form -->
	<div class="bg-white rounded-lg shadow-md p-6 mb-4">
		<h3 class="text-sm font-medium text-gray-900 mb-4">Send Invitation</h3>
		<form onsubmit={handleCreateInvitation}>
			<div class="grid grid-cols-1 md:grid-cols-4 gap-4">
				<div class="md:col-span-2">
					<label for="inviteEmail" class="block text-sm font-medium text-gray-700 mb-1">
						Email
					</label>
					<input
						id="inviteEmail"
						type="email"
						bind:value={inviteEmail}
						placeholder="user@example.com"
						required
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						class:border-red-500={fieldErrors.email}
					/>
					{#if fieldErrors.email}
						<p class="mt-1 text-sm text-red-600">{fieldErrors.email}</p>
					{/if}
				</div>
				<div>
					<label for="inviteRole" class="block text-sm font-medium text-gray-700 mb-1">
						Role
					</label>
					<select
						id="inviteRole"
						bind:value={inviteRole}
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					>
						{#each availableRoles as role}
							<option value={role}>{role.charAt(0).toUpperCase() + role.slice(1)}</option>
						{/each}
					</select>
					{#if fieldErrors.role}
						<p class="mt-1 text-sm text-red-600">{fieldErrors.role}</p>
					{/if}
				</div>
				<div class="flex items-end">
					<button
						type="submit"
						disabled={isCreating}
						class="w-full px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
					>
						{isCreating ? 'Sending...' : 'Send Invitation'}
					</button>
				</div>
			</div>
			{#if createError}
				<div class="mt-3 rounded-md bg-red-50 p-3">
					<p class="text-sm text-red-800">{createError}</p>
				</div>
			{/if}
		</form>
	</div>

	<!-- Invitations table -->
	<div class="bg-white rounded-lg shadow-md overflow-hidden">
		{#if isLoading}
			<div class="flex justify-center items-center py-8">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
			</div>
		{:else if invitations.length === 0}
			<div class="text-center py-8">
				<p class="text-gray-500">No invitations yet</p>
			</div>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-gray-200">
					<thead class="bg-gray-50">
						<tr>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Email
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Role
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Status
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Invited By
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Expires
							</th>
							<th
								class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Actions
							</th>
						</tr>
					</thead>
					<tbody class="bg-white divide-y divide-gray-200">
						{#each invitations as invitation (invitation.id)}
							<tr class="hover:bg-gray-50">
								<td class="px-6 py-4 whitespace-nowrap">
									<div class="text-sm text-gray-900">{invitation.email}</div>
									<div class="text-xs text-gray-500">{formatDate(invitation.createdAt)}</div>
								</td>
								<td class="px-6 py-4 whitespace-nowrap">
									<Badge variant={getRoleVariant(invitation.role)}>
										{invitation.role.charAt(0).toUpperCase() + invitation.role.slice(1)}
									</Badge>
								</td>
								<td class="px-6 py-4 whitespace-nowrap">
									<Badge variant={getStatusVariant(invitation.status)}>
										{invitation.status.charAt(0).toUpperCase() + invitation.status.slice(1)}
									</Badge>
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
									{invitation.invitedBy || '-'}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
									{#if invitation.status === 'pending'}
										{formatRelativeExpiry(invitation.expiresAt)}
									{:else}
										{formatDate(invitation.expiresAt)}
									{/if}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
									{#if invitation.status === 'pending'}
										<button
											onclick={() => (revokeTarget = invitation)}
											class="text-red-600 hover:text-red-900"
										>
											Revoke
										</button>
									{:else if invitation.status === 'expired' || invitation.status === 'revoked'}
										<button
											onclick={() => handleReInvite(invitation)}
											class="text-blue-600 hover:text-blue-900"
										>
											Re-invite
										</button>
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>

			<Pagination
				{currentPage}
				{totalPages}
				totalItems={total}
				{pageSize}
				onPageChange={handlePageChange}
			/>
		{/if}
	</div>
</div>

<!-- Revoke confirmation modal -->
{#if revokeTarget}
	<DeleteConfirmModal
		show={true}
		resourceType="Invitation"
		resourceName={revokeTarget.email}
		onConfirm={handleRevoke}
		onCancel={() => (revokeTarget = null)}
		loading={isRevoking}
		warningMessage="Revoke invitation for {revokeTarget.email}? They will no longer be able to register with this link."
	/>
{/if}
