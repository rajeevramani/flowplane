<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { ArrowLeft, Loader2, Lock, RotateCcw, Plus, Trash2 } from 'lucide-svelte';
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

	// Rotate modal state
	let showRotateModal = $state(false);
	let isRotating = $state(false);
	let rotateError = $state<string | null>(null);
	let rotateExpiresAt = $state('');

	// Rotate config fields (per secret type)
	let genericSecret = $state('');
	let certificateChain = $state('');
	let privateKey = $state('');
	let rotatePassword = $state('');
	let ocspStaple = $state('');
	let trustedCa = $state('');
	let matchSubjectAltNames = $state<{ match_type: string; value: string }[]>([]);
	let crl = $state('');
	let onlyVerifyLeafCertCrl = $state(false);
	let sessionTicketKeys = $state<{ name: string; key: string }[]>([{ name: '', key: '' }]);

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

	// --- Rotate modal helpers ---

	function openRotateModal() {
		rotateError = null;
		rotateExpiresAt = secret?.expires_at ? formatDateTimeForInput(secret.expires_at) : '';
		// Reset config fields
		genericSecret = '';
		certificateChain = '';
		privateKey = '';
		rotatePassword = '';
		ocspStaple = '';
		trustedCa = '';
		matchSubjectAltNames = [];
		crl = '';
		onlyVerifyLeafCertCrl = false;
		sessionTicketKeys = [{ name: '', key: '' }];
		showRotateModal = true;
	}

	function closeRotateModal() {
		showRotateModal = false;
		rotateError = null;
	}

	function addSanMatcher() {
		matchSubjectAltNames = [...matchSubjectAltNames, { match_type: 'exact', value: '' }];
	}

	function removeSanMatcher(index: number) {
		matchSubjectAltNames = matchSubjectAltNames.filter((_, i) => i !== index);
	}

	function addSessionTicketKey() {
		sessionTicketKeys = [...sessionTicketKeys, { name: '', key: '' }];
	}

	function removeSessionTicketKey(index: number) {
		sessionTicketKeys = sessionTicketKeys.filter((_, i) => i !== index);
	}

	function buildRotateConfiguration(): Record<string, unknown> {
		if (!secret) return {};
		switch (secret.secret_type) {
			case 'generic_secret':
				return { type: 'generic_secret', secret: genericSecret };
			case 'tls_certificate': {
				const tlsConfig: Record<string, unknown> = {
					type: 'tls_certificate',
					certificate_chain: certificateChain,
					private_key: privateKey
				};
				if (rotatePassword) tlsConfig.password = rotatePassword;
				if (ocspStaple) tlsConfig.ocsp_staple = ocspStaple;
				return tlsConfig;
			}
			case 'certificate_validation_context': {
				const cvcConfig: Record<string, unknown> = {
					type: 'certificate_validation_context',
					trusted_ca: trustedCa
				};
				if (matchSubjectAltNames.length > 0) {
					cvcConfig.match_subject_alt_names = matchSubjectAltNames;
				}
				if (crl) cvcConfig.crl = crl;
				if (onlyVerifyLeafCertCrl) cvcConfig.only_verify_leaf_cert_crl = true;
				return cvcConfig;
			}
			case 'session_ticket_keys':
				return {
					type: 'session_ticket_keys',
					keys: sessionTicketKeys.filter((k) => k.name && k.key)
				};
			default:
				return {};
		}
	}

	function validateRotateForm(): string | null {
		if (!secret) return 'Secret not loaded';
		switch (secret.secret_type) {
			case 'generic_secret':
				if (!genericSecret.trim()) return 'Secret value is required';
				break;
			case 'tls_certificate':
				if (!certificateChain.trim()) return 'Certificate chain is required';
				if (!privateKey.trim()) return 'Private key is required';
				break;
			case 'certificate_validation_context':
				if (!trustedCa.trim()) return 'Trusted CA is required';
				break;
			case 'session_ticket_keys': {
				const validKeys = sessionTicketKeys.filter((k) => k.name && k.key);
				if (validKeys.length === 0) return 'At least one session ticket key is required';
				break;
			}
		}
		return null;
	}

	async function handleRotate() {
		rotateError = null;
		const validationError = validateRotateForm();
		if (validationError) {
			rotateError = validationError;
			return;
		}

		isRotating = true;
		try {
			const result = await apiClient.rotateSecret(currentTeam, getSecretId(), {
				configuration: buildRotateConfiguration(),
				expires_at: formatExpiresAt(rotateExpiresAt)
			});
			secret = result;
			expiresAt = result.expires_at ? formatDateTimeForInput(result.expires_at) : '';
			showRotateModal = false;
		} catch (e) {
			console.error('Rotate secret failed:', e);
			rotateError = e instanceof Error ? e.message : 'Failed to rotate secret';
		} finally {
			isRotating = false;
		}
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
						Secret values cannot be viewed after creation. Use <strong>Rotate</strong> to replace the secret value with a new one.
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
			<div class="flex gap-3">
				<Button onclick={handleDelete} variant="danger" disabled={isSubmitting || isRotating}>Delete Secret</Button>
				{#if !secret.backend}
					<Button onclick={openRotateModal} variant="secondary" disabled={isSubmitting || isRotating}>
						<RotateCcw class="h-4 w-4 mr-2" />
						Rotate
					</Button>
				{/if}
			</div>
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

<!-- Rotate Secret Modal -->
{#if showRotateModal && secret}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
		onclick={(e) => { if (e.target === e.currentTarget) closeRotateModal(); }}
	>
		<div
			class="bg-white rounded-lg shadow-xl p-6 max-w-2xl w-full mx-4 max-h-[90vh] overflow-y-auto"
			onclick={(e) => e.stopPropagation()}
		>
			<div class="flex items-center gap-3 mb-4">
				<RotateCcw class="h-5 w-5 text-blue-600" />
				<h2 class="text-lg font-semibold text-gray-900">Rotate Secret</h2>
			</div>

			<p class="text-sm text-gray-600 mb-4">
				Provide new secret configuration for <strong>{secret.name}</strong>. This will replace the current value and bump the version to v{secret.version + 1}.
			</p>

			{#if rotateError}
				<div class="mb-4 bg-red-50 border border-red-200 rounded-md p-3">
					<p class="text-sm text-red-800">{rotateError}</p>
				</div>
			{/if}

			<!-- Type-specific config form -->
			<div class="space-y-4 mb-6">
				{#if secret.secret_type === 'generic_secret'}
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							New Secret Value <span class="text-red-500">*</span>
						</label>
						<textarea
							bind:value={genericSecret}
							placeholder="Base64-encoded secret value"
							rows="4"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono text-sm"
						></textarea>
						<p class="text-xs text-gray-500 mt-1">Enter the new secret value (base64-encoded)</p>
					</div>
				{:else if secret.secret_type === 'tls_certificate'}
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Certificate Chain <span class="text-red-500">*</span>
						</label>
						<textarea
							bind:value={certificateChain}
							placeholder="-----BEGIN CERTIFICATE-----&#10;...&#10;-----END CERTIFICATE-----"
							rows="6"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono text-sm"
						></textarea>
					</div>
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Private Key <span class="text-red-500">*</span>
						</label>
						<textarea
							bind:value={privateKey}
							placeholder="-----BEGIN PRIVATE KEY-----&#10;...&#10;-----END PRIVATE KEY-----"
							rows="6"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono text-sm"
						></textarea>
					</div>
					<div class="grid grid-cols-2 gap-4">
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Password (optional)</label>
							<input
								type="password"
								bind:value={rotatePassword}
								placeholder="For encrypted private keys"
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
						</div>
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">OCSP Staple (optional)</label>
							<input
								type="text"
								bind:value={ocspStaple}
								placeholder="Base64-encoded OCSP response"
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							/>
						</div>
					</div>
				{:else if secret.secret_type === 'certificate_validation_context'}
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Trusted CA <span class="text-red-500">*</span>
						</label>
						<textarea
							bind:value={trustedCa}
							placeholder="-----BEGIN CERTIFICATE-----&#10;...&#10;-----END CERTIFICATE-----"
							rows="6"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono text-sm"
						></textarea>
					</div>
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Subject Alternative Name Matchers (optional)
						</label>
						{#each matchSubjectAltNames as matcher, index}
							<div class="flex gap-2 mb-2">
								<select
									bind:value={matcher.match_type}
									class="w-1/3 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								>
									<option value="exact">Exact</option>
									<option value="prefix">Prefix</option>
									<option value="suffix">Suffix</option>
									<option value="safe_regex">Regex</option>
									<option value="contains">Contains</option>
								</select>
								<input
									type="text"
									bind:value={matcher.value}
									placeholder={matcher.match_type === 'safe_regex' ? 'Pattern' : 'Value'}
									class="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<button
									type="button"
									onclick={() => removeSanMatcher(index)}
									class="p-2 text-red-600 hover:bg-red-50 rounded-md transition-colors"
								>
									<Trash2 class="h-4 w-4" />
								</button>
							</div>
						{/each}
						<button
							type="button"
							onclick={addSanMatcher}
							class="flex items-center gap-1 text-sm text-blue-600 hover:text-blue-700"
						>
							<Plus class="h-4 w-4" />
							Add Matcher
						</button>
					</div>
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">CRL (optional)</label>
						<textarea
							bind:value={crl}
							placeholder="PEM-encoded Certificate Revocation List"
							rows="4"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono text-sm"
						></textarea>
					</div>
					<div class="flex items-center gap-2">
						<input
							type="checkbox"
							id="rotateOnlyVerifyLeafCertCrl"
							bind:checked={onlyVerifyLeafCertCrl}
							class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
						/>
						<label for="rotateOnlyVerifyLeafCertCrl" class="text-sm text-gray-700">
							Only verify leaf certificate CRL
						</label>
					</div>
				{:else if secret.secret_type === 'session_ticket_keys'}
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Session Ticket Keys <span class="text-red-500">*</span>
						</label>
						{#each sessionTicketKeys as ticketKey, index}
							<div class="flex gap-2 mb-2">
								<input
									type="text"
									bind:value={ticketKey.name}
									placeholder="Key name"
									class="w-1/3 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
								<input
									type="text"
									bind:value={ticketKey.key}
									placeholder="Base64-encoded key (80 bytes)"
									class="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono text-sm"
								/>
								{#if sessionTicketKeys.length > 1}
									<button
										type="button"
										onclick={() => removeSessionTicketKey(index)}
										class="p-2 text-red-600 hover:bg-red-50 rounded-md transition-colors"
									>
										<Trash2 class="h-4 w-4" />
									</button>
								{/if}
							</div>
						{/each}
						<button
							type="button"
							onclick={addSessionTicketKey}
							class="flex items-center gap-1 text-sm text-blue-600 hover:text-blue-700"
						>
							<Plus class="h-4 w-4" />
							Add Key
						</button>
						<p class="text-xs text-gray-500 mt-1">Each key should be 80 bytes, base64-encoded</p>
					</div>
				{/if}

				<!-- Expiration for rotation -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">New Expiration (optional)</label>
					<input
						type="datetime-local"
						bind:value={rotateExpiresAt}
						class="w-full md:w-64 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">Update the expiration date with the rotation</p>
				</div>
			</div>

			<!-- Modal actions -->
			<div class="flex justify-end gap-3 pt-4 border-t border-gray-200">
				<Button variant="ghost" onclick={closeRotateModal} disabled={isRotating}>Cancel</Button>
				<Button variant="primary" onclick={handleRotate} disabled={isRotating}>
					{#if isRotating}
						<Loader2 class="h-4 w-4 mr-2 animate-spin" />
						Rotating...
					{:else}
						<RotateCcw class="h-4 w-4 mr-2" />
						Rotate Secret
					{/if}
				</Button>
			</div>
		</div>
	</div>
{/if}
