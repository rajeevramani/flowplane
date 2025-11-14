<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { PersonalAccessToken, TokenSecretResponse } from '$lib/api/types';

	let isLoading = $state(true);
	let tokens = $state<PersonalAccessToken[]>([]);
	let error = $state<string | null>(null);
	let toast = $state<{ message: string; type: 'success' | 'error' } | null>(null);

	// Create token modal state
	let showCreateModal = $state(false);
	let createForm = $state({
		name: '',
		description: '',
		scopes: [] as string[],
		expiresAt: ''
	});
	let isCreating = $state(false);
	let createError = $state<string | null>(null);

	// Token secret display state
	let showSecretModal = $state(false);
	let tokenSecret = $state<TokenSecretResponse | null>(null);
	let secretAcknowledged = $state(false);
	let secretCopied = $state(false);

	// Revoke confirmation state
	let showRevokeModal = $state(false);
	let tokenToRevoke = $state<PersonalAccessToken | null>(null);
	let isRevoking = $state(false);

	// Rotate confirmation state
	let showRotateModal = $state(false);
	let tokenToRotate = $state<PersonalAccessToken | null>(null);
	let isRotating = $state(false);

	// Available scopes (grouped by category)
	const scopeGroups = [
		{
			category: 'Tokens',
			scopes: [
				{ value: 'tokens:read', label: 'Read tokens' },
				{ value: 'tokens:write', label: 'Create/update tokens' }
			]
		},
		{
			category: 'Clusters',
			scopes: [
				{ value: 'clusters:read', label: 'Read clusters' },
				{ value: 'clusters:write', label: 'Create/update clusters' }
			]
		},
		{
			category: 'Routes',
			scopes: [
				{ value: 'routes:read', label: 'Read routes' },
				{ value: 'routes:write', label: 'Create/update routes' }
			]
		},
		{
			category: 'Listeners',
			scopes: [
				{ value: 'listeners:read', label: 'Read listeners' },
				{ value: 'listeners:write', label: 'Create/update listeners' }
			]
		},
		{
			category: 'API Definitions',
			scopes: [
				{ value: 'api_definitions:read', label: 'Read API definitions' },
				{ value: 'api_definitions:write', label: 'Create/update API definitions' }
			]
		}
	];

	onMount(async () => {
		await loadTokens();
	});

	async function loadTokens() {
		try {
			isLoading = true;
			error = null;
			tokens = await apiClient.listTokens(100, 0);
		} catch (e: any) {
			error = e.message || 'Failed to load tokens';
			if (e.message?.includes('Unauthorized')) {
				goto('/login');
			}
		} finally {
			isLoading = false;
		}
	}

	function openCreateModal() {
		showCreateModal = true;
		createForm = { name: '', description: '', scopes: [], expiresAt: '' };
		createError = null;
	}

	function closeCreateModal() {
		showCreateModal = false;
		createForm = { name: '', description: '', scopes: [], expiresAt: '' };
		createError = null;
	}

	function toggleScope(scope: string) {
		if (createForm.scopes.includes(scope)) {
			createForm.scopes = createForm.scopes.filter((s) => s !== scope);
		} else {
			createForm.scopes = [...createForm.scopes, scope];
		}
	}

	async function handleCreateToken() {
		if (!createForm.name || createForm.scopes.length === 0) {
			createError = 'Name and at least one scope are required';
			return;
		}

		try {
			isCreating = true;
			createError = null;

			const response = await apiClient.createToken({
				name: createForm.name,
				description: createForm.description || undefined,
				expiresAt: createForm.expiresAt || null,
				scopes: createForm.scopes
			});

			// Show the token secret once
			tokenSecret = response;
			secretAcknowledged = false;
			secretCopied = false;
			showCreateModal = false;
			showSecretModal = true;

			// Reload token list
			await loadTokens();
			showToast('Token created successfully', 'success');
		} catch (e: any) {
			createError = e.message || 'Failed to create token';
		} finally {
			isCreating = false;
		}
	}

	function closeSecretModal() {
		if (!secretAcknowledged) {
			alert('Please acknowledge that you have saved the token before closing.');
			return;
		}
		showSecretModal = false;
		tokenSecret = null;
		secretAcknowledged = false;
		secretCopied = false;
	}

	async function copyTokenSecret() {
		if (tokenSecret) {
			try {
				await navigator.clipboard.writeText(tokenSecret.token);
				secretCopied = true;
				showToast('Token copied to clipboard', 'success');
			} catch (e) {
				showToast('Failed to copy token', 'error');
			}
		}
	}

	function openRevokeModal(token: PersonalAccessToken) {
		tokenToRevoke = token;
		showRevokeModal = true;
	}

	function closeRevokeModal() {
		tokenToRevoke = null;
		showRevokeModal = false;
	}

	async function handleRevokeToken() {
		if (!tokenToRevoke) return;

		try {
			isRevoking = true;
			await apiClient.revokeToken(tokenToRevoke.id);
			await loadTokens();
			showToast(`Token "${tokenToRevoke.name}" revoked successfully`, 'success');
			closeRevokeModal();
		} catch (e: any) {
			showToast(e.message || 'Failed to revoke token', 'error');
		} finally {
			isRevoking = false;
		}
	}

	function openRotateModal(token: PersonalAccessToken) {
		tokenToRotate = token;
		showRotateModal = true;
	}

	function closeRotateModal() {
		tokenToRotate = null;
		showRotateModal = false;
	}

	async function handleRotateToken() {
		if (!tokenToRotate) return;

		try {
			isRotating = true;
			const response = await apiClient.rotateToken(tokenToRotate.id);

			// Show the new token secret
			tokenSecret = response;
			secretAcknowledged = false;
			secretCopied = false;
			showRotateModal = false;
			showSecretModal = true;

			await loadTokens();
			showToast(`Token "${tokenToRotate.name}" rotated successfully`, 'success');
		} catch (e: any) {
			showToast(e.message || 'Failed to rotate token', 'error');
		} finally {
			isRotating = false;
		}
	}

	function showToast(message: string, type: 'success' | 'error') {
		toast = { message, type };
		setTimeout(() => {
			toast = null;
		}, 5000);
	}

	function formatDate(dateStr: string | null): string {
		if (!dateStr) return 'Never';
		return new Date(dateStr).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}

	function formatScopes(scopes: string[]): string {
		if (scopes.length === 0) return 'None';
		if (scopes.length <= 2) return scopes.join(', ');
		return `${scopes.slice(0, 2).join(', ')} + ${scopes.length - 2} more`;
	}

	function getStatusColor(status: string): string {
		switch (status) {
			case 'Active':
				return 'bg-green-100 text-green-800';
			case 'Revoked':
				return 'bg-red-100 text-red-800';
			case 'Expired':
				return 'bg-gray-100 text-gray-800';
			default:
				return 'bg-gray-100 text-gray-800';
		}
	}
</script>

<div class="min-h-screen bg-gray-50">
	<!-- Navigation -->
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a href="/dashboard" class="text-blue-600 hover:text-blue-800" aria-label="Back to dashboard">
						<svg
							class="h-6 w-6"
							fill="none"
							viewBox="0 0 24 24"
							stroke="currentColor"
						>
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M10 19l-7-7m0 0l7-7m-7 7h18"
							/>
						</svg>
					</a>
					<h1 class="text-xl font-bold text-gray-900">Personal Access Tokens</h1>
				</div>
				<button
					onclick={openCreateModal}
					class="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 transition-colors"
				>
					Create Token
				</button>
			</div>
		</div>
	</nav>

	<main class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
		<!-- Error message -->
		{#if error}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800">{error}</p>
			</div>
		{/if}

		<!-- Loading state -->
		{#if isLoading}
			<div class="flex items-center justify-center py-12">
				<div class="text-gray-600">Loading tokens...</div>
			</div>
		{:else if tokens.length === 0}
			<!-- Empty state -->
			<div class="bg-white rounded-lg shadow-md p-12 text-center">
				<svg
					class="mx-auto h-12 w-12 text-gray-400"
					fill="none"
					viewBox="0 0 24 24"
					stroke="currentColor"
				>
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						stroke-width="2"
						d="M15 7a2 2 0 012 2m4 0a6 6 0 01-7.743 5.743L11 17H9v2H7v2H4a1 1 0 01-1-1v-2.586a1 1 0 01.293-.707l5.964-5.964A6 6 0 1121 9z"
					/>
				</svg>
				<h3 class="mt-4 text-lg font-medium text-gray-900">No tokens yet</h3>
				<p class="mt-2 text-sm text-gray-600">
					Get started by creating a new personal access token.
				</p>
				<button
					onclick={openCreateModal}
					class="mt-6 px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 transition-colors"
				>
					Create Your First Token
				</button>
			</div>
		{:else}
			<!-- Tokens table -->
			<div class="bg-white rounded-lg shadow-md overflow-hidden overflow-x-auto">
				<table class="min-w-full divide-y divide-gray-200">
					<thead class="bg-gray-50">
						<tr>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider min-w-[200px]"
							>
								Name
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider min-w-[250px]"
							>
								Description
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider min-w-[100px]"
							>
								Status
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider min-w-[200px]"
							>
								Scopes
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider min-w-[120px]"
							>
								Last Used
							</th>
							<th
								class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider min-w-[120px]"
							>
								Expires
							</th>
							<th
								class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider min-w-[120px]"
							>
								Actions
							</th>
						</tr>
					</thead>
					<tbody class="bg-white divide-y divide-gray-200">
						{#each tokens as token (token.id)}
							<tr class="hover:bg-gray-50">
								<td class="px-6 py-4 whitespace-nowrap">
									<div class="text-sm font-medium text-gray-900">{token.name}</div>
								</td>
								<td class="px-6 py-4">
									<div class="text-sm text-gray-600 max-w-xs truncate">
										{token.description || '-'}
									</div>
								</td>
								<td class="px-6 py-4 whitespace-nowrap">
									<span
										class="px-2 inline-flex text-xs leading-5 font-semibold rounded-full {getStatusColor(
											token.status
										)}"
									>
										{token.status}
									</span>
								</td>
								<td class="px-6 py-4">
									<div class="text-sm text-gray-600" title={token.scopes.join(', ')}>
										{formatScopes(token.scopes)}
									</div>
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
									{formatDate(token.lastUsedAt)}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-600">
									{formatDate(token.expiresAt)}
								</td>
								<td class="px-6 py-4 whitespace-nowrap text-right text-sm font-medium">
									<div class="flex justify-end gap-2">
										{#if token.status === 'Active'}
											<button
												onclick={() => openRotateModal(token)}
												class="text-blue-600 hover:text-blue-900"
												title="Rotate token"
											>
												Rotate
											</button>
											<button
												onclick={() => openRevokeModal(token)}
												class="text-red-600 hover:text-red-900"
												title="Revoke token"
											>
												Revoke
											</button>
										{/if}
									</div>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</main>

	<!-- Create Token Modal -->
	{#if showCreateModal}
		<div class="fixed inset-0 bg-gray-600 bg-opacity-50 flex items-center justify-center p-4 z-50">
			<div class="bg-white rounded-lg shadow-xl max-w-2xl w-full max-h-[90vh] overflow-y-auto">
				<div class="px-6 py-4 border-b border-gray-200">
					<h2 class="text-xl font-semibold text-gray-900">Create Personal Access Token</h2>
				</div>

				<div class="px-6 py-4">
					{#if createError}
						<div class="mb-4 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
							<p class="text-red-800">{createError}</p>
						</div>
					{/if}

					<form onsubmit={(e) => { e.preventDefault(); handleCreateToken(); }}>
						<!-- Name field -->
						<div class="mb-4">
							<label for="token-name" class="block text-sm font-medium text-gray-700 mb-2">
								Name <span class="text-red-500">*</span>
							</label>
							<input
								id="token-name"
								type="text"
								bind:value={createForm.name}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								placeholder="e.g., ci-token"
								required
							/>
						</div>

						<!-- Description field -->
						<div class="mb-4">
							<label for="token-description" class="block text-sm font-medium text-gray-700 mb-2">
								Description
							</label>
							<textarea
								id="token-description"
								bind:value={createForm.description}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								placeholder="What is this token for?"
								rows="2"
							></textarea>
						</div>

						<!-- Expiration field -->
						<div class="mb-4">
							<label for="token-expiration" class="block text-sm font-medium text-gray-700 mb-2">
								Expiration Date (Optional)
							</label>
							<input
								id="token-expiration"
								type="datetime-local"
								bind:value={createForm.expiresAt}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
						</div>

						<!-- Scopes field -->
						<div class="mb-6">
							<div class="block text-sm font-medium text-gray-700 mb-2">
								Scopes <span class="text-red-500">*</span>
							</div>
							<div class="space-y-4">
								{#each scopeGroups as group}
									<div>
										<h4 class="text-sm font-medium text-gray-900 mb-2">{group.category}</h4>
										<div class="space-y-2 pl-4">
											{#each group.scopes as scope}
												<label class="flex items-center">
													<input
														type="checkbox"
														checked={createForm.scopes.includes(scope.value)}
														onchange={() => toggleScope(scope.value)}
														class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
													/>
													<span class="ml-2 text-sm text-gray-700">{scope.label}</span>
													<span class="ml-2 text-xs text-gray-500">({scope.value})</span>
												</label>
											{/each}
										</div>
									</div>
								{/each}
							</div>
						</div>

						<!-- Actions -->
						<div class="flex justify-end gap-3 pt-4 border-t border-gray-200">
							<button
								type="button"
								onclick={closeCreateModal}
								class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
								disabled={isCreating}
							>
								Cancel
							</button>
							<button
								type="submit"
								class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50"
								disabled={isCreating}
							>
								{isCreating ? 'Creating...' : 'Create Token'}
							</button>
						</div>
					</form>
				</div>
			</div>
		</div>
	{/if}

	<!-- Token Secret Modal -->
	{#if showSecretModal && tokenSecret}
		<div class="fixed inset-0 bg-gray-600 bg-opacity-50 flex items-center justify-center p-4 z-50">
			<div class="bg-white rounded-lg shadow-xl max-w-2xl w-full">
				<div class="px-6 py-4 border-b border-gray-200">
					<h2 class="text-xl font-semibold text-gray-900">Your Token Secret</h2>
				</div>

				<div class="px-6 py-4">
					<div class="bg-yellow-50 border-l-4 border-yellow-400 p-4 mb-4">
						<div class="flex">
							<svg class="h-5 w-5 text-yellow-400" fill="currentColor" viewBox="0 0 20 20">
								<path
									fill-rule="evenodd"
									d="M8.257 3.099c.765-1.36 2.722-1.36 3.486 0l5.58 9.92c.75 1.334-.213 2.98-1.742 2.98H4.42c-1.53 0-2.493-1.646-1.743-2.98l5.58-9.92zM11 13a1 1 0 11-2 0 1 1 0 012 0zm-1-8a1 1 0 00-1 1v3a1 1 0 002 0V6a1 1 0 00-1-1z"
									clip-rule="evenodd"
								/>
							</svg>
							<div class="ml-3">
								<p class="text-sm text-yellow-800 font-medium">
									Important: This is the only time you'll see this token!
								</p>
								<p class="mt-1 text-sm text-yellow-700">
									Make sure to copy and save it securely. You won't be able to see it again.
								</p>
							</div>
						</div>
					</div>

					<!-- Token display -->
					<div class="mb-4">
						<div class="block text-sm font-medium text-gray-700 mb-2">Token</div>
						<div class="relative">
							<textarea
								readonly
								value={tokenSecret.token}
								class="w-full px-3 py-2 border border-gray-300 rounded-md bg-gray-50 font-mono text-sm"
								rows="3"
							></textarea>
							<button
								onclick={copyTokenSecret}
								class="absolute top-2 right-2 px-3 py-1 text-sm bg-blue-600 text-white rounded hover:bg-blue-700"
							>
								{secretCopied ? 'Copied!' : 'Copy'}
							</button>
						</div>
					</div>

					<!-- Acknowledgment checkbox -->
					<div class="mb-4">
						<label class="flex items-center">
							<input
								type="checkbox"
								bind:checked={secretAcknowledged}
								class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
							/>
							<span class="ml-2 text-sm text-gray-700">
								I have saved this token in a secure location
							</span>
						</label>
					</div>

					<!-- Close button -->
					<div class="flex justify-end">
						<button
							onclick={closeSecretModal}
							class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50"
							disabled={!secretAcknowledged}
						>
							Close
						</button>
					</div>
				</div>
			</div>
		</div>
	{/if}

	<!-- Revoke Confirmation Modal -->
	{#if showRevokeModal && tokenToRevoke}
		<div class="fixed inset-0 bg-gray-600 bg-opacity-50 flex items-center justify-center p-4 z-50">
			<div class="bg-white rounded-lg shadow-xl max-w-md w-full">
				<div class="px-6 py-4 border-b border-gray-200">
					<h2 class="text-xl font-semibold text-gray-900">Revoke Token</h2>
				</div>

				<div class="px-6 py-4">
					<p class="text-sm text-gray-700 mb-4">
						Are you sure you want to revoke the token "<strong>{tokenToRevoke.name}</strong>"?
						This action cannot be undone.
					</p>

					<div class="flex justify-end gap-3">
						<button
							onclick={closeRevokeModal}
							class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
							disabled={isRevoking}
						>
							Cancel
						</button>
						<button
							onclick={handleRevokeToken}
							class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700 disabled:opacity-50"
							disabled={isRevoking}
						>
							{isRevoking ? 'Revoking...' : 'Revoke Token'}
						</button>
					</div>
				</div>
			</div>
		</div>
	{/if}

	<!-- Rotate Confirmation Modal -->
	{#if showRotateModal && tokenToRotate}
		<div class="fixed inset-0 bg-gray-600 bg-opacity-50 flex items-center justify-center p-4 z-50">
			<div class="bg-white rounded-lg shadow-xl max-w-md w-full">
				<div class="px-6 py-4 border-b border-gray-200">
					<h2 class="text-xl font-semibold text-gray-900">Rotate Token</h2>
				</div>

				<div class="px-6 py-4">
					<p class="text-sm text-gray-700 mb-4">
						Are you sure you want to rotate the token "<strong>{tokenToRotate.name}</strong>"?
						The old token will be invalidated and a new one will be generated.
					</p>

					<div class="flex justify-end gap-3">
						<button
							onclick={closeRotateModal}
							class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
							disabled={isRotating}
						>
							Cancel
						</button>
						<button
							onclick={handleRotateToken}
							class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50"
							disabled={isRotating}
						>
							{isRotating ? 'Rotating...' : 'Rotate Token'}
						</button>
					</div>
				</div>
			</div>
		</div>
	{/if}

	<!-- Toast Notification -->
	{#if toast}
		<div class="fixed bottom-4 right-4 z-50 animate-fade-in">
			<div
				class="px-6 py-4 rounded-lg shadow-lg {toast.type === 'success'
					? 'bg-green-500'
					: 'bg-red-500'} text-white"
			>
				<div class="flex items-center gap-3">
					{#if toast.type === 'success'}
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M5 13l4 4L19 7"
							/>
						</svg>
					{:else}
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M6 18L18 6M6 6l12 12"
							/>
						</svg>
					{/if}
					<span>{toast.message}</span>
				</div>
			</div>
		</div>
	{/if}
</div>

<style>
	@keyframes fade-in {
		from {
			opacity: 0;
			transform: translateY(1rem);
		}
		to {
			opacity: 1;
			transform: translateY(0);
		}
	}

	.animate-fade-in {
		animation: fade-in 0.3s ease-out;
	}
</style>
