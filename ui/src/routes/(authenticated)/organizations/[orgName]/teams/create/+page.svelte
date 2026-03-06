<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import type { CreateTeamRequest } from '$lib/api/types';
	import { isOrgAdmin } from '$lib/stores/org';

	let orgName = $derived($page.params.orgName ?? '');

	let isSubmitting = $state(false);
	let submitError = $state<string | null>(null);

	let formData = $state({
		name: '',
		displayName: '',
		description: ''
	});

	let errors = $state<Record<string, string>>({});

	onMount(async () => {
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!isOrgAdmin(sessionInfo.orgScopes)) {
				goto(`/organizations/${orgName}/teams`);
			}
		} catch {
			goto('/login');
		}
	});

	function validateForm(): boolean {
		const newErrors: Record<string, string> = {};

		if (!formData.name.trim()) {
			newErrors.name = 'Name is required';
		} else if (!/^[a-z0-9][a-z0-9-]*[a-z0-9]$|^[a-z0-9]$/.test(formData.name)) {
			newErrors.name = 'Name must be lowercase alphanumeric and hyphens only (cannot start or end with hyphen)';
		} else if (formData.name.length > 255) {
			newErrors.name = 'Name must be 255 characters or less';
		}

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

	async function handleSubmit() {
		if (!validateForm()) return;

		isSubmitting = true;
		submitError = null;

		try {
			const request: CreateTeamRequest = {
				name: formData.name,
				displayName: formData.displayName,
				description: formData.description || null
			};

			const team = await apiClient.createOrgTeam(orgName, request);
			goto(`/organizations/${orgName}/teams/${team.name}`);
		} catch (err: unknown) {
			submitError = err instanceof Error ? err.message : 'Failed to create team';
		} finally {
			isSubmitting = false;
		}
	}
</script>

<div class="min-h-screen bg-gray-50">
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="w-full px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a
						href="/organizations/{orgName}/teams"
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
		{#if submitError}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{submitError}</p>
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
							bind:value={formData.name}
							placeholder="my-team"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono {errors.name
								? 'border-red-500'
								: ''}"
						/>
						<p class="mt-1 text-xs text-gray-500">
							Lowercase letters, numbers, and hyphens only. Cannot start or end with a hyphen.
						</p>
						{#if errors.name}
							<p class="mt-1 text-sm text-red-600">{errors.name}</p>
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
							placeholder="My Team"
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
							placeholder="Optional description for this team"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.description
								? 'border-red-500'
								: ''}"
						></textarea>
						{#if errors.description}
							<p class="mt-1 text-sm text-red-600">{errors.description}</p>
						{/if}
					</div>
				</div>

				<div class="mt-6 flex justify-end gap-3">
					<a
						href="/organizations/{orgName}/teams"
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
					>
						Cancel
					</a>
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
