<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Plus, Edit, Trash2, Lock, Key, Shield, Clock } from 'lucide-svelte';
	import type { SecretResponse, SecretType, SecretBackend, SessionInfoResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import { adminSummary, adminSummaryLoading, adminSummaryError, getAdminSummary } from '$lib/stores/adminSummary';
	import AdminResourceSummary from '$lib/components/AdminResourceSummary.svelte';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	let sessionInfo = $state<SessionInfoResponse | null>(null);

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);
	let searchQuery = $state('');
	let typeFilter = $state<SecretType | ''>('');
	let storageFilter = $state<'all' | 'database' | 'external'>('all');
	let currentTeam = $state<string>('');

	// Data
	let secrets = $state<SecretResponse[]>([]);

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		if (currentTeam && currentTeam !== value) {
			currentTeam = value;
			loadData();
		} else {
			currentTeam = value;
		}
	});

	onMount(async () => {
		sessionInfo = await apiClient.getSessionInfo();
		if (sessionInfo.isPlatformAdmin) {
			try { await getAdminSummary(); } catch { /* handled by store */ }
			isLoading = false;
			return;
		}
		await loadData();
	});

	async function loadData() {
		isLoading = true;
		error = null;

		try {
			const secretsData = await apiClient.listSecrets(currentTeam);
			secrets = secretsData;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load secrets';
			console.error('Failed to load secrets:', e);
		} finally {
			isLoading = false;
		}
	}

	// Calculate stats
	let stats = $derived({
		totalSecrets: secrets.length,
		tlsCertificates: secrets.filter(s => s.secret_type === 'tls_certificate').length,
		genericSecrets: secrets.filter(s => s.secret_type === 'generic_secret').length,
		externalReferences: secrets.filter(s => s.backend).length
	});

	// Filter secrets
	let filteredSecrets = $derived(
		secrets
			.filter(secret => {
				// Type filter
				if (typeFilter && secret.secret_type !== typeFilter) return false;
				// Storage filter
				if (storageFilter === 'database' && secret.backend) return false;
				if (storageFilter === 'external' && !secret.backend) return false;
				// Search filter
				if (searchQuery) {
					const query = searchQuery.toLowerCase();
					return (
						secret.name.toLowerCase().includes(query) ||
						(secret.description && secret.description.toLowerCase().includes(query))
					);
				}
				return true;
			})
	);

	// Format secret type for display
	function formatSecretType(type: SecretType): string {
		const typeMap: Record<SecretType, string> = {
			generic_secret: 'Generic Secret',
			tls_certificate: 'TLS Certificate',
			certificate_validation_context: 'Validation Context',
			session_ticket_keys: 'Session Ticket Keys'
		};
		return typeMap[type] || type;
	}

	// Get badge variant for secret type
	function getSecretTypeBadgeVariant(type: SecretType): 'green' | 'blue' | 'purple' | 'orange' {
		const variantMap: Record<SecretType, 'green' | 'blue' | 'purple' | 'orange'> = {
			generic_secret: 'green',
			tls_certificate: 'blue',
			certificate_validation_context: 'purple',
			session_ticket_keys: 'orange'
		};
		return variantMap[type] || 'gray';
	}

	// Format storage method for display
	function formatStorageMethod(secret: SecretResponse): string {
		if (secret.backend) {
			const backendMap: Record<SecretBackend, string> = {
				vault: 'Vault',
				aws_secrets_manager: 'AWS',
				gcp_secret_manager: 'GCP'
			};
			return backendMap[secret.backend as SecretBackend] || secret.backend;
		}
		return 'Database';
	}

	// Get badge variant for storage method
	function getStorageBadgeVariant(secret: SecretResponse): 'gray' | 'purple' | 'orange' | 'blue' {
		if (!secret.backend) return 'gray';
		const variantMap: Record<SecretBackend, 'purple' | 'orange' | 'blue'> = {
			vault: 'purple',
			aws_secrets_manager: 'orange',
			gcp_secret_manager: 'blue'
		};
		return variantMap[secret.backend as SecretBackend] || 'gray';
	}

	// Navigate to create page
	function handleCreate() {
		goto('/secrets/create');
	}

	// Navigate to edit page
	function handleEdit(secretId: string) {
		goto(`/secrets/${encodeURIComponent(secretId)}/edit`);
	}

	// Delete secret
	async function handleDelete(secret: SecretResponse) {
		if (!confirm(`Are you sure you want to delete the secret "${secret.name}"? This action cannot be undone.`)) {
			return;
		}

		actionError = null;

		try {
			await apiClient.deleteSecret(currentTeam, secret.id);
			await loadData();
		} catch (err) {
			actionError = err instanceof Error ? err.message : 'Failed to delete secret';
		}
	}

	// Format date
	function formatDate(dateStr: string): string {
		const date = new Date(dateStr);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}

	// Check if secret is expiring soon (within 30 days)
	function isExpiringSoon(secret: SecretResponse): boolean {
		if (!secret.expires_at) return false;
		const expiresAt = new Date(secret.expires_at);
		const now = new Date();
		const thirtyDaysFromNow = new Date(now.getTime() + 30 * 24 * 60 * 60 * 1000);
		return expiresAt <= thirtyDaysFromNow && expiresAt > now;
	}

	// Check if secret is expired
	function isExpired(secret: SecretResponse): boolean {
		if (!secret.expires_at) return false;
		const expiresAt = new Date(secret.expires_at);
		return expiresAt <= new Date();
	}
</script>

{#if sessionInfo?.isPlatformAdmin}
<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Secrets</h1>
		<p class="mt-2 text-sm text-gray-600">Platform-wide secret summary across all organizations and teams.</p>
	</div>
	{#if $adminSummaryLoading}
		<div class="flex items-center justify-center py-12"><div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div></div>
	{:else if $adminSummaryError}
		<div class="bg-red-50 border border-red-200 rounded-md p-4"><p class="text-sm text-red-800">{$adminSummaryError}</p></div>
	{:else if $adminSummary}
		<AdminResourceSummary summary={$adminSummary} highlightResource="secrets" />
	{/if}
</div>
{:else}
<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Secrets</h1>
		<p class="mt-2 text-sm text-gray-600">
			Manage SDS secrets for the <span class="font-medium">{currentTeam}</span> team
		</p>
	</div>

	<!-- Action Buttons -->
	<div class="mb-6">
		<Button onclick={handleCreate} variant="primary">
			<Plus class="h-4 w-4 mr-2" />
			Create Secret
		</Button>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-4 gap-4 mb-6">
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Secrets</p>
					<p class="text-2xl font-bold text-gray-900">{stats.totalSecrets}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<Lock class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">TLS Certificates</p>
					<p class="text-2xl font-bold text-gray-900">{stats.tlsCertificates}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<Shield class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Generic Secrets</p>
					<p class="text-2xl font-bold text-gray-900">{stats.genericSecrets}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<Key class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">External References</p>
					<p class="text-2xl font-bold text-gray-900">{stats.externalReferences}</p>
				</div>
				<div class="p-3 bg-purple-100 rounded-lg">
					<Clock class="h-6 w-6 text-purple-600" />
				</div>
			</div>
		</div>
	</div>

	<!-- Filters -->
	<div class="mb-6 flex flex-wrap gap-4">
		<input
			type="text"
			bind:value={searchQuery}
			placeholder="Search by name or description..."
			class="w-full md:w-96 px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		/>

		<select
			bind:value={typeFilter}
			class="px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		>
			<option value="">All Types</option>
			<option value="generic_secret">Generic Secret</option>
			<option value="tls_certificate">TLS Certificate</option>
			<option value="certificate_validation_context">Validation Context</option>
			<option value="session_ticket_keys">Session Ticket Keys</option>
		</select>

		<select
			bind:value={storageFilter}
			class="px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		>
			<option value="all">All Storage</option>
			<option value="database">Database</option>
			<option value="external">External Reference</option>
		</select>
	</div>

	<!-- Action Error -->
	{#if actionError}
		<div class="bg-red-50 border border-red-200 rounded-md p-4 mb-6">
			<p class="text-sm text-red-800">{actionError}</p>
		</div>
	{/if}

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading secrets...</span>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filteredSecrets.length === 0}
		<!-- Empty State -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
			<Lock class="h-12 w-12 text-gray-400 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">
				{searchQuery || typeFilter || storageFilter !== 'all' ? 'No secrets found' : 'No secrets yet'}
			</h3>
			<p class="text-sm text-gray-600 mb-6">
				{searchQuery || typeFilter || storageFilter !== 'all'
					? 'Try adjusting your filters'
					: 'Get started by creating a new secret'}
			</p>
			{#if !searchQuery && !typeFilter && storageFilter === 'all'}
				<Button onclick={handleCreate} variant="primary">
					<Plus class="h-4 w-4 mr-2" />
					Create Secret
				</Button>
			{/if}
		</div>
	{:else}
		<!-- Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Name
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Type
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Storage
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Description
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Expires
						</th>
						<th class="px-6 py-3 text-right text-xs font-medium text-gray-500 uppercase tracking-wider">
							Actions
						</th>
					</tr>
				</thead>
				<tbody class="bg-white divide-y divide-gray-200">
					{#each filteredSecrets as secret}
						<tr class="hover:bg-gray-50 transition-colors">
							<!-- Name -->
							<td class="px-6 py-4">
								<div class="flex flex-col">
									<span class="text-sm font-medium text-gray-900">{secret.name}</span>
									<span class="text-xs text-gray-500 font-mono">{secret.id}</span>
								</div>
							</td>

							<!-- Type -->
							<td class="px-6 py-4">
								<Badge variant={getSecretTypeBadgeVariant(secret.secret_type)}>
									{formatSecretType(secret.secret_type)}
								</Badge>
							</td>

							<!-- Storage -->
							<td class="px-6 py-4">
								<Badge variant={getStorageBadgeVariant(secret)}>
									{formatStorageMethod(secret)}
								</Badge>
								{#if secret.reference}
									<span class="block text-xs text-gray-500 mt-1 truncate max-w-[150px]" title={secret.reference}>
										{secret.reference}
									</span>
								{/if}
							</td>

							<!-- Description -->
							<td class="px-6 py-4">
								<span class="text-sm text-gray-600">
									{secret.description || '-'}
								</span>
							</td>

							<!-- Expires -->
							<td class="px-6 py-4">
								{#if secret.expires_at}
									{#if isExpired(secret)}
										<span class="text-sm text-red-600 font-medium">Expired</span>
									{:else if isExpiringSoon(secret)}
										<span class="text-sm text-amber-600 font-medium">
											{formatDate(secret.expires_at)}
										</span>
									{:else}
										<span class="text-sm text-gray-600">{formatDate(secret.expires_at)}</span>
									{/if}
								{:else}
									<span class="text-sm text-gray-400">Never</span>
								{/if}
							</td>

							<!-- Actions -->
							<td class="px-6 py-4 text-right">
								<div class="flex justify-end gap-2">
									<button
										onclick={() => handleEdit(secret.id)}
										class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
										title="Edit secret"
									>
										<Edit class="h-4 w-4" />
									</button>
									<button
										onclick={() => handleDelete(secret)}
										class="p-2 text-red-600 hover:bg-red-50 rounded-md transition-colors"
										title="Delete secret"
									>
										<Trash2 class="h-4 w-4" />
									</button>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		<!-- Pagination Placeholder -->
		{#if filteredSecrets.length > 50}
			<div class="mt-4 flex justify-center">
				<p class="text-sm text-gray-600">Showing {filteredSecrets.length} secrets</p>
			</div>
		{/if}
	{/if}
</div>
{/if}
