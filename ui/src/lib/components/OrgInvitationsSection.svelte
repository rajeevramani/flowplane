<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { inviteMemberSchema } from '$lib/schemas/auth';
	import { ZodError } from 'zod';
	import { isSystemAdmin } from '$lib/stores/org';
	import type { InvitableRole, OrgMembershipResponse } from '$lib/api/types';
	import Badge from './Badge.svelte';

	interface Props {
		orgName: string;
		orgId: string;
		userScopes: string[];
	}

	let { orgName, orgId, userScopes }: Props = $props();

	// State
	let members = $state<OrgMembershipResponse[]>([]);
	let isLoading = $state(true);
	let error = $state<string | null>(null);

	// Invite form state
	let inviteEmail = $state('');
	let inviteFirstName = $state('');
	let inviteLastName = $state('');
	let inviteRole = $state<InvitableRole>('member');
	let invitePassword = $state('');
	let isCreating = $state(false);
	let createError = $state<string | null>(null);
	let fieldErrors = $state<Record<string, string>>({});

	// Success state
	let successMessage = $state<string | null>(null);

	// Determine available roles based on user permissions
	const isAdmin = $derived(isSystemAdmin(userScopes));
	const availableRoles = $derived<InvitableRole[]>(
		isAdmin ? ['admin', 'member', 'viewer'] : ['member', 'viewer']
	);

	function getRoleVariant(role: string): 'blue' | 'gray' | 'purple' {
		switch (role) {
			case 'admin':
			case 'owner':
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

	async function loadMembers() {
		isLoading = true;
		error = null;
		try {
			members = await apiClient.listOrgMembers(orgId);
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to load members';
		} finally {
			isLoading = false;
		}
	}

	async function handleInvite(event: Event) {
		event.preventDefault();
		createError = null;
		fieldErrors = {};
		successMessage = null;
		isCreating = true;

		try {
			inviteMemberSchema.parse({
				email: inviteEmail,
				role: inviteRole,
				firstName: inviteFirstName,
				lastName: inviteLastName,
				initialPassword: invitePassword || undefined
			});

			const response = await apiClient.inviteOrgMember(orgId, {
				email: inviteEmail,
				role: inviteRole,
				firstName: inviteFirstName,
				lastName: inviteLastName,
				initialPassword: invitePassword || undefined
			});

			const action = response.userCreated ? 'invited' : 'added';
			successMessage = `User ${response.email} ${action} as ${response.role}`;

			inviteEmail = '';
			inviteFirstName = '';
			inviteLastName = '';
			inviteRole = 'member';
			invitePassword = '';

			await loadMembers();
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
				createError = 'Failed to invite member';
			}
		} finally {
			isCreating = false;
		}
	}

	function dismissSuccess() {
		successMessage = null;
	}

	// Initial load
	$effect(() => {
		loadMembers();
	});
</script>

<div class="mt-8">
	<div class="flex items-center justify-between mb-4">
		<h2 class="text-lg font-semibold text-gray-900">Invite Member</h2>
	</div>

	{#if error}
		<div class="mb-4 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
			<p class="text-red-800 text-sm">{error}</p>
		</div>
	{/if}

	<!-- Success banner -->
	{#if successMessage}
		<div class="mb-4 bg-green-50 border border-green-200 rounded-md p-4">
			<div class="flex items-center justify-between">
				<p class="text-sm font-medium text-green-800">{successMessage}</p>
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
	{/if}

	<!-- Invite form -->
	<div class="bg-white rounded-lg shadow-md p-6 mb-4">
		<h3 class="text-sm font-medium text-gray-900 mb-4">Invite by Email</h3>
		<form onsubmit={handleInvite}>
			<div class="grid grid-cols-1 md:grid-cols-6 gap-4">
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
					<label for="inviteFirstName" class="block text-sm font-medium text-gray-700 mb-1">
						First Name
					</label>
					<input
						id="inviteFirstName"
						type="text"
						bind:value={inviteFirstName}
						placeholder="Jane"
						required
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						class:border-red-500={fieldErrors.firstName}
					/>
					{#if fieldErrors.firstName}
						<p class="mt-1 text-sm text-red-600">{fieldErrors.firstName}</p>
					{/if}
				</div>
				<div>
					<label for="inviteLastName" class="block text-sm font-medium text-gray-700 mb-1">
						Last Name
					</label>
					<input
						id="inviteLastName"
						type="text"
						bind:value={inviteLastName}
						placeholder="Doe"
						required
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						class:border-red-500={fieldErrors.lastName}
					/>
					{#if fieldErrors.lastName}
						<p class="mt-1 text-sm text-red-600">{fieldErrors.lastName}</p>
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
			</div>
			<div class="grid grid-cols-1 md:grid-cols-6 gap-4 mt-4">
				<div class="md:col-span-2">
					<label for="invitePassword" class="block text-sm font-medium text-gray-700 mb-1">
						Initial Password <span class="text-gray-400">(optional)</span>
					</label>
					<input
						id="invitePassword"
						type="password"
						bind:value={invitePassword}
						placeholder="Set password (local dev)"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						class:border-red-500={fieldErrors.initialPassword}
					/>
					{#if fieldErrors.initialPassword}
						<p class="mt-1 text-sm text-red-600">{fieldErrors.initialPassword}</p>
					{:else}
						<p class="mt-1 text-xs text-gray-400">
							Skip if SMTP is configured — Zitadel will email a setup link.
						</p>
					{/if}
				</div>
				<div class="flex items-end">
					<button
						type="submit"
						disabled={isCreating}
						class="w-full px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
					>
						{isCreating ? 'Inviting...' : 'Invite'}
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

	<!-- Members from invite list -->
	<div class="bg-white rounded-lg shadow-md overflow-hidden">
		{#if isLoading}
			<div class="flex justify-center items-center py-8">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
			</div>
		{:else if members.length === 0}
			<div class="text-center py-8">
				<p class="text-gray-500">No members yet. Invite someone above.</p>
			</div>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-gray-200">
					<thead class="bg-gray-50">
						<tr>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider"
							>
								Member
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
									<Badge variant={getRoleVariant(member.role)}>
										{member.role.charAt(0).toUpperCase() + member.role.slice(1)}
									</Badge>
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
									{formatDate(member.createdAt)}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</div>
</div>
