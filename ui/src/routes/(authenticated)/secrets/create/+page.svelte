<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { ArrowLeft, Loader2, Plus, Trash2 } from 'lucide-svelte';
	import { selectedTeam } from '$lib/stores/team';
	import type { SecretType, SecretBackend } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';

	// Form state
	let currentTeam = $state<string>('');
	let error = $state<string | null>(null);
	let isSubmitting = $state(false);

	// Basic fields
	let secretName = $state('');
	let secretDescription = $state('');
	let secretType = $state<SecretType>('generic_secret');
	let expiresAt = $state('');

	// Storage method toggle: 'direct' or 'external'
	let storageMethod = $state<'direct' | 'external'>('direct');

	// External reference fields
	let backend = $state<SecretBackend>('vault');
	let reference = $state('');
	let referenceVersion = $state('');

	// Direct storage configurations for each type
	// GenericSecret
	let genericSecret = $state('');

	// TlsCertificate
	let certificateChain = $state('');
	let privateKey = $state('');
	let password = $state('');
	let ocspStaple = $state('');

	// CertificateValidationContext
	let trustedCa = $state('');
	let matchSubjectAltNames = $state<{ match_type: string; value: string }[]>([]);
	let crl = $state('');
	let onlyVerifyLeafCertCrl = $state(false);

	// SessionTicketKeys
	let sessionTicketKeys = $state<{ name: string; key: string }[]>([{ name: '', key: '' }]);

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Get placeholder for backend reference
	function getReferencePlaceholder(be: SecretBackend): string {
		const placeholders: Record<SecretBackend, string> = {
			vault: 'secret/data/myapp/credentials',
			aws_secrets_manager: 'arn:aws:secretsmanager:us-west-2:123456789:secret:myapp/credentials',
			gcp_secret_manager: 'projects/my-project/secrets/my-secret/versions/latest'
		};
		return placeholders[be];
	}

	// Add a new SAN matcher
	function addSanMatcher() {
		matchSubjectAltNames = [...matchSubjectAltNames, { match_type: 'exact', value: '' }];
	}

	// Remove a SAN matcher
	function removeSanMatcher(index: number) {
		matchSubjectAltNames = matchSubjectAltNames.filter((_, i) => i !== index);
	}

	// Add a session ticket key
	function addSessionTicketKey() {
		sessionTicketKeys = [...sessionTicketKeys, { name: '', key: '' }];
	}

	// Remove a session ticket key
	function removeSessionTicketKey(index: number) {
		sessionTicketKeys = sessionTicketKeys.filter((_, i) => i !== index);
	}

	// Build configuration based on secret type
	// Backend uses tagged enum: #[serde(tag = "type", rename_all = "snake_case")]
	function buildConfiguration(): Record<string, unknown> {
		switch (secretType) {
			case 'generic_secret':
				return { type: 'generic_secret', secret: genericSecret };
			case 'tls_certificate':
				const tlsConfig: Record<string, unknown> = {
					type: 'tls_certificate',
					certificate_chain: certificateChain,
					private_key: privateKey
				};
				if (password) tlsConfig.password = password;
				if (ocspStaple) tlsConfig.ocsp_staple = ocspStaple;
				return tlsConfig;
			case 'certificate_validation_context':
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
			case 'session_ticket_keys':
				return {
					type: 'session_ticket_keys',
					keys: sessionTicketKeys.filter(k => k.name && k.key)
				};
			default:
				return {};
		}
	}

	// Validate form
	function validateForm(): string | null {
		if (!secretName.trim()) {
			return 'Secret name is required';
		}
		if (secretName.length > 255) {
			return 'Secret name must be 255 characters or less';
		}

		if (storageMethod === 'external') {
			if (!reference.trim()) {
				return 'Backend reference is required';
			}
		} else {
			// Validate direct storage based on type
			switch (secretType) {
				case 'generic_secret':
					if (!genericSecret.trim()) {
						return 'Secret value is required';
					}
					break;
				case 'tls_certificate':
					if (!certificateChain.trim()) {
						return 'Certificate chain is required';
					}
					if (!privateKey.trim()) {
						return 'Private key is required';
					}
					break;
				case 'certificate_validation_context':
					if (!trustedCa.trim()) {
						return 'Trusted CA is required';
					}
					break;
				case 'session_ticket_keys':
					const validKeys = sessionTicketKeys.filter(k => k.name && k.key);
					if (validKeys.length === 0) {
						return 'At least one session ticket key is required';
					}
					break;
			}
		}

		return null;
	}

	// Convert datetime-local value to ISO 8601 format
	function formatExpiresAt(value: string): string | undefined {
		if (!value) return undefined;
		// datetime-local gives "2025-12-20T17:04", convert to ISO 8601
		const date = new Date(value);
		if (isNaN(date.getTime())) return undefined;
		return date.toISOString();
	}

	// Handle form submission
	async function handleSubmit() {
		error = null;
		const validationError = validateForm();
		if (validationError) {
			error = validationError;
			return;
		}

		isSubmitting = true;

		try {
			if (storageMethod === 'external') {
				// Create reference-based secret
				await apiClient.createSecretReference(currentTeam, {
					name: secretName.trim(),
					secret_type: secretType,
					description: secretDescription.trim() || undefined,
					backend: backend,
					reference: reference.trim(),
					reference_version: referenceVersion.trim() || undefined,
					expires_at: formatExpiresAt(expiresAt)
				});
			} else {
				// Create direct storage secret
				await apiClient.createSecret(currentTeam, {
					name: secretName.trim(),
					secret_type: secretType,
					description: secretDescription.trim() || undefined,
					configuration: buildConfiguration(),
					expires_at: formatExpiresAt(expiresAt)
				});
			}
			goto('/secrets');
		} catch (e) {
			console.error('Create secret failed:', e);
			error = e instanceof Error ? e.message : 'Failed to create secret';
		} finally {
			isSubmitting = false;
		}
	}

	function handleCancel() {
		goto('/secrets');
	}
</script>

<div class="max-w-4xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
	<!-- Page Header with Back Button -->
	<div class="mb-6">
		<div class="flex items-center gap-4 mb-2">
			<button
				onclick={handleCancel}
				class="p-2 text-gray-400 hover:text-gray-600 hover:bg-gray-100 rounded-md transition-colors"
			>
				<ArrowLeft class="w-5 h-5" />
			</button>
			<div>
				<h1 class="text-2xl font-bold text-gray-900">Create Secret</h1>
				<p class="mt-1 text-sm text-gray-600">Create a new SDS secret for your Envoy configuration</p>
			</div>
		</div>
	</div>

	<!-- Error Message -->
	{#if error}
		<div class="mb-6 bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{/if}

	<!-- Basic Information -->
	<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
		<h2 class="text-lg font-semibold text-gray-900 mb-4">Basic Information</h2>
		<div class="space-y-4">
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Secret Name <span class="text-red-500">*</span>
				</label>
				<input
					type="text"
					bind:value={secretName}
					placeholder="e.g., api-oauth-token"
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
				<p class="text-xs text-gray-500 mt-1">
					A unique name to identify this secret within your team
				</p>
			</div>

			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Description</label>
				<textarea
					bind:value={secretDescription}
					placeholder="Optional description of what this secret is for"
					rows="2"
					class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				></textarea>
			</div>

			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Team</label>
				<input
					type="text"
					value={currentTeam}
					disabled
					class="w-full px-3 py-2 border border-gray-200 rounded-md bg-gray-50 text-gray-500"
				/>
				<p class="text-xs text-gray-500 mt-1">Secrets are scoped to your current team</p>
			</div>
		</div>
	</div>

	<!-- Storage Method Toggle -->
	<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
		<h2 class="text-lg font-semibold text-gray-900 mb-4">Storage Method</h2>
		<div class="flex rounded-lg border border-gray-300 overflow-hidden">
			<button
				type="button"
				onclick={() => (storageMethod = 'direct')}
				class="flex-1 px-4 py-3 text-sm font-medium transition-colors {storageMethod === 'direct'
					? 'bg-blue-600 text-white'
					: 'bg-white text-gray-700 hover:bg-gray-50'}"
			>
				Direct Storage
			</button>
			<button
				type="button"
				onclick={() => (storageMethod = 'external')}
				class="flex-1 px-4 py-3 text-sm font-medium transition-colors border-l border-gray-300 {storageMethod === 'external'
					? 'bg-blue-600 text-white'
					: 'bg-white text-gray-700 hover:bg-gray-50'}"
			>
				External Reference
			</button>
		</div>
		<p class="text-xs text-gray-500 mt-2">
			{#if storageMethod === 'direct'}
				Secret values will be encrypted and stored in the database (AES-256-GCM)
			{:else}
				Reference secrets from external providers (Vault, AWS, GCP)
			{/if}
		</p>
	</div>

	<!-- Secret Type Selector -->
	<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
		<h2 class="text-lg font-semibold text-gray-900 mb-4">Secret Type</h2>
		<div>
			<label class="block text-sm font-medium text-gray-700 mb-1">
				Select Secret Type <span class="text-red-500">*</span>
			</label>
			<select
				bind:value={secretType}
				class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
			>
				<option value="generic_secret">Generic Secret - OAuth tokens, API keys, HMAC secrets</option>
				<option value="tls_certificate">TLS Certificate - Public/private key pairs for TLS</option>
				<option value="certificate_validation_context">Certificate Validation Context - CA certificates for peer verification</option>
				<option value="session_ticket_keys">Session Ticket Keys - For TLS session resumption</option>
			</select>
		</div>
	</div>

	<!-- Configuration Section -->
	<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
		<h2 class="text-lg font-semibold text-gray-900 mb-4">Configuration</h2>

		{#if storageMethod === 'external'}
			<!-- External Reference Configuration -->
			<div class="space-y-4">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Backend <span class="text-red-500">*</span>
					</label>
					<select
						bind:value={backend}
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					>
						<option value="vault">HashiCorp Vault</option>
						<option value="aws_secrets_manager">AWS Secrets Manager</option>
						<option value="gcp_secret_manager">GCP Secret Manager</option>
					</select>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Reference <span class="text-red-500">*</span>
					</label>
					<input
						type="text"
						bind:value={reference}
						placeholder={getReferencePlaceholder(backend)}
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Full path or ARN to the secret in the external backend
					</p>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">Version (optional)</label>
					<input
						type="text"
						bind:value={referenceVersion}
						placeholder="e.g., 1 or AWSCURRENT"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Specific version to use, or leave empty for latest
					</p>
				</div>
			</div>
		{:else}
			<!-- Direct Storage Configuration -->
			{#if secretType === 'generic_secret'}
				<div class="space-y-4">
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-1">
							Secret Value <span class="text-red-500">*</span>
						</label>
						<textarea
							bind:value={genericSecret}
							placeholder="Base64-encoded secret value"
							rows="4"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 font-mono text-sm"
						></textarea>
						<p class="text-xs text-gray-500 mt-1">
							Enter the secret value (base64-encoded)
						</p>
					</div>
				</div>
			{:else if secretType === 'tls_certificate'}
				<div class="space-y-4">
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
						<p class="text-xs text-gray-500 mt-1">PEM-encoded certificate chain</p>
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
						<p class="text-xs text-gray-500 mt-1">PEM-encoded private key</p>
					</div>

					<div class="grid grid-cols-2 gap-4">
						<div>
							<label class="block text-sm font-medium text-gray-700 mb-1">Password (optional)</label>
							<input
								type="password"
								bind:value={password}
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
				</div>
			{:else if secretType === 'certificate_validation_context'}
				<div class="space-y-4">
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
						<p class="text-xs text-gray-500 mt-1">PEM-encoded CA certificate for verification</p>
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
							id="onlyVerifyLeafCertCrl"
							bind:checked={onlyVerifyLeafCertCrl}
							class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
						/>
						<label for="onlyVerifyLeafCertCrl" class="text-sm text-gray-700">
							Only verify leaf certificate CRL
						</label>
					</div>
				</div>
			{:else if secretType === 'session_ticket_keys'}
				<div class="space-y-4">
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
					<p class="text-xs text-gray-500">
						Each key should be 80 bytes, base64-encoded
					</p>
				</div>
			{/if}
		{/if}
	</div>

	<!-- Expiration -->
	<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6 mb-6">
		<h2 class="text-lg font-semibold text-gray-900 mb-4">Expiration (Optional)</h2>
		<div>
			<label class="block text-sm font-medium text-gray-700 mb-1">Expires At</label>
			<input
				type="datetime-local"
				bind:value={expiresAt}
				class="w-full md:w-64 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
			/>
			<p class="text-xs text-gray-500 mt-1">
				Set an expiration date to receive warnings about expiring secrets
			</p>
		</div>
	</div>

	<!-- Action Buttons -->
	<div class="flex justify-end gap-3">
		<Button onclick={handleCancel} variant="secondary" disabled={isSubmitting}>Cancel</Button>
		<Button onclick={handleSubmit} variant="primary" disabled={isSubmitting}>
			{#if isSubmitting}
				<Loader2 class="h-4 w-4 mr-2 animate-spin" />
				Creating...
			{:else}
				Create Secret
			{/if}
		</Button>
	</div>
</div>
