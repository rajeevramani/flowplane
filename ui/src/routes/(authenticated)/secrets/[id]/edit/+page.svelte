<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { ArrowLeft, Loader2, AlertTriangle, Lock, Info } from 'lucide-svelte';
	import type { SecretResponse, SecretType, SecretBackend } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';

	// Page state
	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let isSubmitting = $state(false);
	let secret = $state<SecretResponse | null>(null);
	let currentTeam = $state<string>('');

	// Form state (editable fields only)
	let secretName = $state('');
	let secretDescription = $state('');
	let expiresAt = $state('');

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	function getSecretId(): string {
		return $page.params.id ?? '';
	}

	onMount(async () => {
		await loadSecret();
	});

	async function loadSecret() {
		isLoading = true;
		error = null;

		try {
			const data = await apiClient.getSecret(currentTeam, getSecretId());
			secret = data;

			// Populate form fields
			secretName = data.name;
			secretDescription = data.description || '';
			expiresAt = data.expires_at ? formatDateTimeForInput(data.expires_at) : '';
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load secret';
		} finally {
			isLoading = false;
		}
	}

	// Format ISO date to datetime-local input format
	function formatDateTimeForInput(isoDate: string): string {
		const date = new Date(isoDate);
		return date.toISOString().slice(0, 16);
	}

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
	function formatStorageMethod(s: SecretResponse): string {
		if (s.backend) {
			const backendMap: Record<SecretBackend, string> = {
				vault: 'HashiCorp Vault',
				aws_secrets_manager: 'AWS Secrets Manager',
				gcp_secret_manager: 'GCP Secret Manager'
			};
			return backendMap[s.backend as SecretBackend] || s.backend;
		}
		return 'Database (Encrypted)';
	}

	// Format date for display
	function formatDate(dateStr: string): string {
		return new Date(dateStr).toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	// Validation
	function validateForm(): string | null {
		if (!secretName.trim()) {
			return 'Secret name is required';
		}
		if (secretName.length > 255) {
			return 'Secret name must be 255 characters or less';
		}
		return null;
	}

	// Convert datetime-local value to ISO 8601 format
	function formatExpiresAt(value: string): string | null {
		if (!value) return null;
		// datetime-local gives "2025-12-20T17:04", convert to ISO 8601
		const date = new Date(value);
		if (isNaN(date.getTime())) return null;
		return date.toISOString();
	}

	async function handleSubmit() {
		error = null;
		const validationError = validateForm();
		if (validationError) {
			error = validationError;
			return;
		}

		isSubmitting = true;
		try {
			await apiClient.updateSecret(currentTeam, getSecretId(), {
				description: secretDescription.trim() || undefined,
				expires_at: formatExpiresAt(expiresAt)
			});
			goto('/secrets');
		} catch (e) {
			console.error('Update secret failed:', e);
			error = e instanceof Error ? e.message : 'Failed to update secret';
		} finally {
			isSubmitting = false;
		}
	}

	async function handleDelete() {
		if (!confirm(`Are you sure you want to delete "${secret?.name}"? This cannot be undone.`)) {
			return;
		}
		try {
			await apiClient.deleteSecret(currentTeam, getSecretId());
			goto('/secrets');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to delete secret';
		}
	}

	function handleCancel() {
		goto('/secrets');
	}
</script>

<div class="max-w-4xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<Loader2 class="w-8 h-8 text-blue-600 animate-spin" />
				<span class="text-sm text-gray-600">Loading secret...</span>
			</div>
		</div>
	{:else if error && !secret}
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
			<Button onclick={handleCancel} variant="secondary" size="sm">Back to Secrets</Button>
		</div>
	{:else if secret}
		<!-- Header -->
		<div class="mb-6">
			<div class="flex items-center gap-4 mb-2">
				<button
					onclick={handleCancel}
					class="p-2 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded-md transition-colors"
				>
					<ArrowLeft class="w-5 h-5" />
				</button>
				<div class="flex-1">
					<div class="flex items-center gap-3">
						<h1 class="text-2xl font-bold text-gray-900">{secret.name}</h1>
						<Badge variant={getSecretTypeBadgeVariant(secret.secret_type)}>
							{formatSecretType(secret.secret_type)}
						</Badge>
						<span class="px-2 py-0.5 rounded text-xs font-medium bg-gray-100 text-gray-600">
							v{secret.version}
						</span>
					</div>
					{#if secret.description}
						<p class="text-sm text-gray-600 mt-1">{secret.description}</p>
					{/if}
				</div>
			</div>
		</div>

		{#if error}
			<div class="mb-6 bg-red-50 border border-red-200 rounded-md p-4">
				<p class="text-sm text-red-800">{error}</p>
			</div>
		{/if}

		<!-- Security Notice -->
		<div class="mb-6 bg-amber-50 border border-amber-200 rounded-md p-4">
			<div class="flex gap-3">
				<Lock class="h-5 w-5 text-amber-600 flex-shrink-0 mt-0.5" />
				<div>
					<h3 class="text-sm font-medium text-amber-800">Secret Values Protected</h3>
					<p class="text-sm text-amber-700 mt-1">
						Secret values cannot be viewed or edited after creation. To update the secret value, create a new secret and update your references.
					</p>
				</div>
			</div>
		</div>

		<!-- Read-Only Information -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Secret Information</h2>
			<div class="grid grid-cols-2 gap-4">
				<div>
					<label class="block text-sm font-medium text-gray-500 mb-1">Secret ID</label>
					<p class="text-sm font-mono text-gray-900">{secret.id}</p>
				</div>
				<div>
					<label class="block text-sm font-medium text-gray-500 mb-1">Secret Type</label>
					<Badge variant={getSecretTypeBadgeVariant(secret.secret_type)}>
						{formatSecretType(secret.secret_type)}
					</Badge>
				</div>
				<div>
					<label class="block text-sm font-medium text-gray-500 mb-1">Storage Method</label>
					<p class="text-sm text-gray-900">{formatStorageMethod(secret)}</p>
					{#if secret.reference}
						<p class="text-xs text-gray-500 font-mono mt-1">{secret.reference}</p>
					{/if}
				</div>
				<div>
					<label class="block text-sm font-medium text-gray-500 mb-1">Team</label>
					<Badge variant="indigo">{secret.team}</Badge>
				</div>
				<div>
					<label class="block text-sm font-medium text-gray-500 mb-1">Created</label>
					<p class="text-sm text-gray-900">{formatDate(secret.created_at)}</p>
				</div>
				<div>
					<label class="block text-sm font-medium text-gray-500 mb-1">Updated</label>
					<p class="text-sm text-gray-900">{formatDate(secret.updated_at)}</p>
				</div>
			</div>
		</div>

		<!-- Editable Fields -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Edit Metadata</h2>
			<div class="space-y-4">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Secret Name
					</label>
					<input
						type="text"
						value={secretName}
						disabled
						class="w-full px-3 py-2 border border-gray-200 rounded-md bg-gray-50 text-gray-500"
					/>
					<p class="text-xs text-gray-500 mt-1">Secret name cannot be changed after creation</p>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Description</label>
					<textarea
						bind:value={secretDescription}
						placeholder="Optional description"
						rows="2"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					></textarea>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Expires At</label>
					<input
						type="datetime-local"
						bind:value={expiresAt}
						class="w-full md:w-64 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Set or update the expiration date
					</p>
				</div>
			</div>
		</div>

		<!-- Metadata -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Version History</h2>
			<div class="grid grid-cols-2 gap-4 text-sm">
				<div>
					<span class="text-gray-500">Version:</span>
					<span class="text-gray-900 ml-2">{secret.version}</span>
				</div>
				<div>
					<span class="text-gray-500">Source:</span>
					<span class="text-gray-900 ml-2 capitalize">{secret.source}</span>
				</div>
			</div>
		</div>

		<!-- Actions -->
		<div class="flex justify-between">
			<Button onclick={handleDelete} variant="danger" disabled={isSubmitting}>Delete Secret</Button>
			<div class="flex gap-3">
				<Button onclick={handleCancel} variant="secondary" disabled={isSubmitting}>Cancel</Button>
				<Button onclick={handleSubmit} variant="primary" disabled={isSubmitting}>
					{#if isSubmitting}
						<Loader2 class="h-4 w-4 mr-2 animate-spin" />
						Saving...
					{:else}
						Save Changes
					{/if}
				</Button>
			</div>
		</div>
	{/if}
</div>
