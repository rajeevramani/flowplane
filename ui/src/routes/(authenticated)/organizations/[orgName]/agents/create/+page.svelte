<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import type { CreateAgentResponse } from '$lib/api/types';
	import { isOrgAdmin } from '$lib/stores/org';

	let orgName = $derived($page.params.orgName ?? '');

	let isSubmitting = $state(false);
	let submitError = $state<string | null>(null);

	// Available teams for the org
	let availableTeams = $state<string[]>([]);
	let isLoadingTeams = $state(true);

	// Form data
	let name = $state('');
	let description = $state('');
	let selectedTeams = $state<string[]>([]);
	let errors = $state<Record<string, string>>({});

	// Credential modal state
	let showCredModal = $state(false);
	let credential = $state<CreateAgentResponse | null>(null);
	let acknowledged = $state(false);
	let copyFeedback = $state<Record<string, boolean>>({});

	// Already-exists state
	let showAlreadyExists = $state(false);
	let alreadyExistsMessage = $state('');

	onMount(async () => {
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!isOrgAdmin(sessionInfo.orgScopes)) {
				goto(`/organizations/${orgName}/agents`);
				return;
			}
			const teamsResp = await apiClient.listOrgTeams(orgName);
			availableTeams = teamsResp.teams.map((t) => t.name);
		} catch {
			goto('/login');
		} finally {
			isLoadingTeams = false;
		}
	});

	function toggleTeam(teamName: string) {
		if (selectedTeams.includes(teamName)) {
			selectedTeams = selectedTeams.filter((t) => t !== teamName);
		} else {
			selectedTeams = [...selectedTeams, teamName];
		}
	}

	function validateForm(): boolean {
		const newErrors: Record<string, string> = {};

		if (!name.trim()) {
			newErrors.name = 'Name is required';
		} else if (name.length < 3 || name.length > 63) {
			newErrors.name = 'Name must be between 3 and 63 characters';
		} else if (!/^[a-z0-9][a-z0-9-]*[a-z0-9]$|^[a-z0-9]$/.test(name)) {
			newErrors.name = 'Name must be lowercase alphanumeric and hyphens only (cannot start or end with hyphen)';
		}

		if (description && description.length > 256) {
			newErrors.description = 'Description must be 256 characters or fewer';
		}

		if (selectedTeams.length === 0) {
			newErrors.teams = 'At least one team must be selected';
		}

		errors = newErrors;
		return Object.keys(newErrors).length === 0;
	}

	async function handleSubmit() {
		if (!validateForm()) return;

		isSubmitting = true;
		submitError = null;
		showAlreadyExists = false;

		try {
			const response = await apiClient.createOrgAgent(orgName, {
				name: name.trim(),
				description: description.trim() || null,
				teams: selectedTeams
			});

			// HTTP 200 = already exists (idempotent)
			if (!response.clientId) {
				alreadyExistsMessage = response.message ?? 'Agent already exists. Credentials were returned at creation time only.';
				showAlreadyExists = true;
			} else {
				// HTTP 201 = created — show credential modal
				credential = response;
				acknowledged = false;
				showCredModal = true;
			}
		} catch (err: unknown) {
			submitError = err instanceof Error ? err.message : 'Failed to create agent';
		} finally {
			isSubmitting = false;
		}
	}

	async function copyToClipboard(value: string, key: string) {
		try {
			await navigator.clipboard.writeText(value);
			copyFeedback = { ...copyFeedback, [key]: true };
			setTimeout(() => {
				copyFeedback = { ...copyFeedback, [key]: false };
			}, 2000);
		} catch {
			// Clipboard not available
		}
	}

	function closeCredModal() {
		if (!acknowledged) return;
		showCredModal = false;
		goto(`/organizations/${orgName}/agents`);
	}
</script>

<div class="min-h-screen bg-gray-50">
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="w-full px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a
						href="/organizations/{orgName}/agents"
						class="text-blue-600 hover:text-blue-800"
						aria-label="Back to agents"
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
					<h1 class="text-xl font-bold text-gray-900">Create Agent</h1>
				</div>
			</div>
		</div>
	</nav>

	<main class="max-w-2xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
		{#if submitError}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{submitError}</p>
			</div>
		{/if}

		{#if showAlreadyExists}
			<div class="mb-6 bg-blue-50 border-l-4 border-blue-500 rounded-md p-4">
				<p class="text-blue-800 text-sm">{alreadyExistsMessage}</p>
				<a
					href="/organizations/{orgName}/agents"
					class="mt-2 inline-block text-sm text-blue-700 underline hover:text-blue-900"
				>
					View agent list
				</a>
			</div>
		{/if}

		<div class="bg-white rounded-lg shadow-md p-6">
			<form
				onsubmit={(e) => {
					e.preventDefault();
					handleSubmit();
				}}
			>
				<div class="space-y-6">
					<!-- Name -->
					<div>
						<label for="name" class="block text-sm font-medium text-gray-700 mb-2">
							Name <span class="text-red-500">*</span>
						</label>
						<input
							id="name"
							type="text"
							bind:value={name}
							placeholder="my-agent"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono {errors.name ? 'border-red-500' : ''}"
						/>
						<p class="mt-1 text-xs text-gray-500">
							3–63 characters. Lowercase letters, numbers, and hyphens only. Cannot start or end with a hyphen.
						</p>
						{#if errors.name}
							<p class="mt-1 text-sm text-red-600">{errors.name}</p>
						{/if}
					</div>

					<!-- Description -->
					<div>
						<label for="description" class="block text-sm font-medium text-gray-700 mb-2">
							Description
						</label>
						<textarea
							id="description"
							bind:value={description}
							rows="2"
							placeholder="Optional description for this agent"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.description ? 'border-red-500' : ''}"
						></textarea>
						{#if errors.description}
							<p class="mt-1 text-sm text-red-600">{errors.description}</p>
						{/if}
					</div>

					<!-- Teams -->
					<div>
						<p class="block text-sm font-medium text-gray-700 mb-2">
							Teams <span class="text-red-500">*</span>
						</p>
						{#if isLoadingTeams}
							<div class="flex items-center gap-2 text-sm text-gray-500">
								<div class="animate-spin rounded-full h-4 w-4 border-b-2 border-blue-600"></div>
								Loading teams…
							</div>
						{:else if availableTeams.length === 0}
							<p class="text-sm text-gray-500">No teams found in this org.</p>
						{:else}
							<div class="grid grid-cols-2 sm:grid-cols-3 gap-2">
								{#each availableTeams as team}
									<label class="flex items-center gap-2 text-sm cursor-pointer">
										<input
											type="checkbox"
											checked={selectedTeams.includes(team)}
											onchange={() => toggleTeam(team)}
											class="rounded border-gray-300 text-blue-600"
										/>
										<span class="font-mono text-gray-700">{team}</span>
									</label>
								{/each}
							</div>
						{/if}
						{#if errors.teams}
							<p class="mt-1 text-sm text-red-600">{errors.teams}</p>
						{/if}
					</div>

					<!-- Permissions note -->
					<div class="bg-blue-50 border border-blue-200 rounded-md p-3">
						<p class="text-sm text-blue-800">
							Permissions are managed via grants after the agent is created.
						</p>
					</div>
				</div>

				<div class="mt-6 flex justify-end gap-3">
					<a
						href="/organizations/{orgName}/agents"
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
					>
						Cancel
					</a>
					<button
						type="submit"
						disabled={isSubmitting}
						class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50"
					>
						{isSubmitting ? 'Creating…' : 'Create Agent'}
					</button>
				</div>
			</form>
		</div>
	</main>
</div>

<!-- Credential display modal -->
{#if showCredModal && credential}
	<div
		class="fixed inset-0 bg-black/50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
	>
		<div class="bg-white rounded-lg shadow-xl p-6 max-w-lg w-full mx-4">
			<!-- Warning banner -->
			<div class="bg-yellow-50 border border-yellow-300 rounded-md p-3 mb-5">
				<p class="text-yellow-800 text-sm font-medium">
					⚠ These credentials are shown only once. Save them now — they cannot be retrieved again.
				</p>
			</div>

			<h2 class="text-lg font-semibold text-gray-900 mb-4">Agent Credentials</h2>

			<div class="space-y-4">
				<!-- Client ID -->
				<div>
					<label class="block text-xs font-medium text-gray-500 uppercase tracking-wider mb-1">
						Client ID
					</label>
					<div class="flex gap-2">
						<input
							type="text"
							readonly
							value={credential.clientId ?? ''}
							class="flex-1 px-3 py-2 border border-gray-300 rounded-md bg-gray-50 font-mono text-sm text-gray-900"
						/>
						<button
							onclick={() => copyToClipboard(credential?.clientId ?? '', 'clientId')}
							class="px-3 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 whitespace-nowrap"
						>
							{copyFeedback.clientId ? 'Copied!' : 'Copy'}
						</button>
					</div>
				</div>

				<!-- Client Secret -->
				<div>
					<label class="block text-xs font-medium text-gray-500 uppercase tracking-wider mb-1">
						Client Secret
					</label>
					<div class="flex gap-2">
						<input
							type="text"
							readonly
							value={credential.clientSecret ?? ''}
							class="flex-1 px-3 py-2 border border-gray-300 rounded-md bg-gray-50 font-mono text-sm text-gray-900"
						/>
						<button
							onclick={() => copyToClipboard(credential?.clientSecret ?? '', 'clientSecret')}
							class="px-3 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 whitespace-nowrap"
						>
							{copyFeedback.clientSecret ? 'Copied!' : 'Copy'}
						</button>
					</div>
				</div>

				<!-- Token Endpoint -->
				<div>
					<label class="block text-xs font-medium text-gray-500 uppercase tracking-wider mb-1">
						Token Endpoint
					</label>
					<div class="flex gap-2">
						<input
							type="text"
							readonly
							value={credential.tokenEndpoint}
							class="flex-1 px-3 py-2 border border-gray-300 rounded-md bg-gray-50 font-mono text-sm text-gray-900"
						/>
						<button
							onclick={() => copyToClipboard(credential?.tokenEndpoint ?? '', 'tokenEndpoint')}
							class="px-3 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 whitespace-nowrap"
						>
							{copyFeedback.tokenEndpoint ? 'Copied!' : 'Copy'}
						</button>
					</div>
				</div>
			</div>

			<!-- Acknowledgment -->
			<div class="mt-5">
				<label class="flex items-start gap-3 cursor-pointer">
					<input
						type="checkbox"
						bind:checked={acknowledged}
						class="mt-0.5 rounded border-gray-300 text-blue-600"
					/>
					<span class="text-sm text-gray-700">
						I have saved these credentials in a secure location
					</span>
				</label>
			</div>

			<div class="mt-5 flex justify-end">
				<button
					onclick={closeCredModal}
					disabled={!acknowledged}
					class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
				>
					Close
				</button>
			</div>
		</div>
	</div>
{/if}
