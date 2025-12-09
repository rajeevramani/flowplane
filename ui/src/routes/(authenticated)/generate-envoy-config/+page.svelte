<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount, onDestroy } from 'svelte';
	import { selectedTeam } from '$lib/stores/team';
	import type { Unsubscriber } from 'svelte/store';
	import type { TeamResponse, MtlsStatusResponse, GenerateCertificateResponse } from '$lib/api/types';
	import hljs from 'highlight.js/lib/core';
	import yaml from 'highlight.js/lib/languages/yaml';
	import json from 'highlight.js/lib/languages/json';
	import 'highlight.js/styles/github-dark.css';

	// Register languages
	hljs.registerLanguage('yaml', yaml);
	hljs.registerLanguage('json', json);

	// State
	let currentTeam = $state('');
	let teamDetails = $state<TeamResponse | null>(null);
	let format = $state<'yaml' | 'json'>('yaml');
	let bootstrapConfig = $state('');
	let highlightedCode = $state('');
	let isLoading = $state(false);
	let error = $state<string | null>(null);
	let copySuccess = $state(false);
	let unsubscribe: Unsubscriber;

	// mTLS state
	let mtlsStatus = $state<MtlsStatusResponse | null>(null);
	let proxyId = $state('');
	let proxyIdError = $state<string | null>(null);
	let isGeneratingCert = $state(false);
	let generatedCertificate = $state<GenerateCertificateResponse | null>(null);
	let certCopySuccess = $state<Record<string, boolean>>({});
	let enableMtlsInBootstrap = $state(true);

	// Default certificate paths
	const DEFAULT_CERT_PATH = '/etc/envoy/certs/client.pem';
	const DEFAULT_KEY_PATH = '/etc/envoy/certs/client-key.pem';
	const DEFAULT_CA_PATH = '/etc/envoy/certs/ca.pem';

	let certPath = $state(DEFAULT_CERT_PATH);
	let keyPath = $state(DEFAULT_KEY_PATH);
	let caPath = $state(DEFAULT_CA_PATH);

	// Proxy ID validation regex
	const PROXY_ID_REGEX = /^[a-zA-Z0-9][a-zA-Z0-9_-]*$/;

	onMount(async () => {
		// Load mTLS status
		await loadMtlsStatus();

		// Subscribe to team changes from shared store
		unsubscribe = selectedTeam.subscribe(async (team) => {
			if (team && team !== currentTeam) {
				currentTeam = team;
				// Reset certificate state on team change
				generatedCertificate = null;
				proxyId = '';
				await Promise.all([loadBootstrapConfig(), loadTeamDetails()]);
			}
		});
	});

	async function loadMtlsStatus() {
		try {
			mtlsStatus = await apiClient.getMtlsStatus();
			// If mTLS is not available, disable mTLS in bootstrap by default
			if (!mtlsStatus.pkiMountConfigured) {
				enableMtlsInBootstrap = false;
			}
		} catch (err) {
			// If we can't get mTLS status, assume it's not configured
			mtlsStatus = null;
		}
	}

	async function loadTeamDetails() {
		if (!currentTeam) {
			teamDetails = null;
			return;
		}

		try {
			const teamsResponse = await apiClient.adminListTeams(100, 0);
			const team = teamsResponse.teams.find((t) => t.name === currentTeam);
			teamDetails = team || null;
		} catch (err) {
			teamDetails = null;
		}
	}

	onDestroy(() => {
		if (unsubscribe) {
			unsubscribe();
		}
	});

	async function loadBootstrapConfig() {
		if (!currentTeam) return;

		isLoading = true;
		error = null;

		try {
			bootstrapConfig = await apiClient.getBootstrapConfig({
				team: currentTeam,
				format,
				mtls: enableMtlsInBootstrap && mtlsStatus?.pkiMountConfigured,
				certPath: enableMtlsInBootstrap ? certPath : undefined,
				keyPath: enableMtlsInBootstrap ? keyPath : undefined,
				caPath: enableMtlsInBootstrap ? caPath : undefined
			});

			// Highlight the code
			const language = format === 'yaml' ? 'yaml' : 'json';
			highlightedCode = hljs.highlight(bootstrapConfig, { language }).value;
		} catch (err: unknown) {
			const message = err instanceof Error ? err.message : 'Failed to load bootstrap configuration';
			error = message;
		} finally {
			isLoading = false;
		}
	}

	function handleFormatChange() {
		loadBootstrapConfig();
	}

	function handleMtlsToggle() {
		loadBootstrapConfig();
	}

	function validateProxyId(value: string): string | null {
		if (!value) {
			return 'Proxy ID is required';
		}
		if (value.length < 3) {
			return 'Proxy ID must be at least 3 characters';
		}
		if (value.length > 64) {
			return 'Proxy ID must be at most 64 characters';
		}
		if (!PROXY_ID_REGEX.test(value)) {
			return 'Proxy ID must start with alphanumeric and contain only alphanumeric characters, hyphens, and underscores';
		}
		return null;
	}

	function handleProxyIdInput(event: Event) {
		const target = event.target as HTMLInputElement;
		proxyId = target.value;
		proxyIdError = validateProxyId(proxyId);
	}

	async function generateCertificate() {
		proxyIdError = validateProxyId(proxyId);
		if (proxyIdError) {
			return;
		}

		isGeneratingCert = true;
		error = null;

		try {
			generatedCertificate = await apiClient.generateProxyCertificate(currentTeam, {
				proxyId
			});

			// Reload bootstrap config with mTLS enabled
			enableMtlsInBootstrap = true;
			await loadBootstrapConfig();
		} catch (err: unknown) {
			const message = err instanceof Error ? err.message : 'Failed to generate certificate';
			error = message;
		} finally {
			isGeneratingCert = false;
		}
	}

	function downloadConfig() {
		const blob = new Blob([bootstrapConfig], { type: 'text/plain' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = `bootstrap-${currentTeam}.${format}`;
		document.body.appendChild(a);
		a.click();
		document.body.removeChild(a);
		URL.revokeObjectURL(url);
	}

	async function copyToClipboard() {
		try {
			await navigator.clipboard.writeText(bootstrapConfig);
			copySuccess = true;
			setTimeout(() => {
				copySuccess = false;
			}, 2000);
		} catch (err) {
			error = 'Failed to copy to clipboard';
		}
	}

	async function copyCertificateField(field: 'certificate' | 'privateKey' | 'caChain') {
		if (!generatedCertificate) return;

		try {
			await navigator.clipboard.writeText(generatedCertificate[field]);
			certCopySuccess = { ...certCopySuccess, [field]: true };
			setTimeout(() => {
				certCopySuccess = { ...certCopySuccess, [field]: false };
			}, 2000);
		} catch (err) {
			error = 'Failed to copy to clipboard';
		}
	}

	function formatExpiryDate(dateString: string): string {
		const date = new Date(dateString);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'long',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit',
			timeZoneName: 'short'
		});
	}

	function getDaysUntilExpiry(dateString: string): number {
		const expiry = new Date(dateString);
		const now = new Date();
		const diffMs = expiry.getTime() - now.getTime();
		return Math.ceil(diffMs / (1000 * 60 * 60 * 24));
	}
</script>

{#if error}
	<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
		<p class="text-red-800 text-sm">{error}</p>
	</div>
{/if}

<div class="grid grid-cols-1 lg:grid-cols-3 gap-6">
	<!-- Configuration Options -->
	<div class="lg:col-span-1">
		<div class="bg-white rounded-lg shadow-md p-6">
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Configuration Options</h2>

			<div class="space-y-4">
				<!-- Team Display (controlled by navbar) -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-2">Team</label>
					<div
						class="px-3 py-2 bg-gray-100 border border-gray-300 rounded-md text-sm text-gray-700"
					>
						{currentTeam || 'No team selected'}
					</div>
					<p class="mt-1 text-xs text-gray-500">Use the navbar team selector to change teams</p>
				</div>

				<!-- Admin Port Info -->
				{#if teamDetails?.envoyAdminPort}
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-2">Envoy Admin Port</label>
						<div
							class="px-3 py-2 bg-gray-100 border border-gray-300 rounded-md text-sm text-gray-700 font-mono"
						>
							{teamDetails.envoyAdminPort}
						</div>
						<p class="mt-1 text-xs text-gray-500">
							Auto-allocated admin interface port for this team
						</p>
					</div>
				{/if}

				<!-- Format Selection -->
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-2">Format</label>
					<div class="space-y-2">
						<label class="flex items-center">
							<input
								type="radio"
								bind:group={format}
								value="yaml"
								onchange={handleFormatChange}
								class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300"
							/>
							<span class="ml-2 text-sm text-gray-700">YAML</span>
						</label>
						<label class="flex items-center">
							<input
								type="radio"
								bind:group={format}
								value="json"
								onchange={handleFormatChange}
								class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300"
							/>
							<span class="ml-2 text-sm text-gray-700">JSON</span>
						</label>
					</div>
				</div>

				<!-- mTLS Configuration Section -->
				{#if mtlsStatus}
					<div class="border-t border-gray-200 pt-4">
						<h3 class="text-sm font-medium text-gray-900 mb-3">mTLS Configuration</h3>

						{#if mtlsStatus.pkiMountConfigured}
							<!-- mTLS is available -->
							<div class="space-y-3">
								<label class="flex items-center">
									<input
										type="checkbox"
										bind:checked={enableMtlsInBootstrap}
										onchange={handleMtlsToggle}
										class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
									/>
									<span class="ml-2 text-sm text-gray-700">Enable mTLS in bootstrap</span>
								</label>

								{#if enableMtlsInBootstrap}
									<div class="ml-6 space-y-2">
										<div>
											<label class="block text-xs font-medium text-gray-600 mb-1"
												>Certificate Path</label
											>
											<input
												type="text"
												bind:value={certPath}
												onblur={loadBootstrapConfig}
												class="w-full px-2 py-1 text-xs border border-gray-300 rounded-md focus:ring-blue-500 focus:border-blue-500"
											/>
										</div>
										<div>
											<label class="block text-xs font-medium text-gray-600 mb-1">Key Path</label>
											<input
												type="text"
												bind:value={keyPath}
												onblur={loadBootstrapConfig}
												class="w-full px-2 py-1 text-xs border border-gray-300 rounded-md focus:ring-blue-500 focus:border-blue-500"
											/>
										</div>
										<div>
											<label class="block text-xs font-medium text-gray-600 mb-1">CA Path</label>
											<input
												type="text"
												bind:value={caPath}
												onblur={loadBootstrapConfig}
												class="w-full px-2 py-1 text-xs border border-gray-300 rounded-md focus:ring-blue-500 focus:border-blue-500"
											/>
										</div>
									</div>
								{/if}
							</div>

							<!-- Proxy ID Input for Certificate Generation -->
							<div class="mt-4 pt-4 border-t border-gray-100">
								<label class="block text-sm font-medium text-gray-700 mb-2">
									Proxy ID
									<span class="text-red-500">*</span>
								</label>
								<input
									type="text"
									value={proxyId}
									oninput={handleProxyIdInput}
									placeholder="e.g., my-proxy-instance"
									class="w-full px-3 py-2 border rounded-md text-sm focus:ring-blue-500 focus:border-blue-500 {proxyIdError
										? 'border-red-500'
										: 'border-gray-300'}"
								/>
								{#if proxyIdError}
									<p class="mt-1 text-xs text-red-600">{proxyIdError}</p>
								{:else}
									<p class="mt-1 text-xs text-gray-500">
										A unique identifier for this proxy instance (e.g., hostname, deployment ID)
									</p>
								{/if}

								<button
									onclick={generateCertificate}
									disabled={isGeneratingCert || !currentTeam || !proxyId || !!proxyIdError}
									class="mt-3 w-full px-4 py-2 text-sm font-medium text-white bg-green-600 rounded-md hover:bg-green-700 disabled:opacity-50 disabled:cursor-not-allowed"
								>
									{isGeneratingCert ? 'Generating...' : 'Generate Certificate'}
								</button>
							</div>
						{:else}
							<!-- mTLS not configured -->
							<div class="bg-amber-50 border-l-4 border-amber-500 rounded-md p-3">
								<p class="text-amber-800 text-sm font-medium">mTLS Not Configured</p>
								<p class="text-amber-700 text-xs mt-1">
									{mtlsStatus.message}
								</p>
							</div>
						{/if}
					</div>
				{/if}

				<!-- Actions -->
				<div class="pt-4 space-y-2">
					<button
						onclick={downloadConfig}
						disabled={!bootstrapConfig || isLoading}
						class="w-full px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
					>
						Download Configuration
					</button>
					<button
						onclick={copyToClipboard}
						disabled={!bootstrapConfig || isLoading}
						class="w-full px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200 disabled:opacity-50 disabled:cursor-not-allowed"
					>
						{copySuccess ? 'Copied!' : 'Copy to Clipboard'}
					</button>
				</div>
			</div>
		</div>

		<!-- Usage Instructions -->
		<div class="mt-6 bg-blue-50 border-l-4 border-blue-500 rounded-md p-4">
			<h3 class="text-sm font-medium text-blue-800 mb-2">How to Use</h3>
			<div class="text-sm text-blue-700 space-y-2">
				{#if generatedCertificate}
					<p>1. Save the certificate files to your proxy</p>
					<p>2. Download or copy the bootstrap configuration</p>
					<p>
						3. Save it as <code class="bg-blue-100 px-1 rounded">bootstrap.yaml</code>
					</p>
					<p>4. Start Envoy with the configuration:</p>
				{:else}
					<p>1. Download or copy the bootstrap configuration</p>
					<p>
						2. Save it as <code class="bg-blue-100 px-1 rounded">bootstrap.yaml</code>
					</p>
					<p>3. Start Envoy with the configuration:</p>
				{/if}
				<pre class="mt-2 p-2 bg-blue-100 rounded text-xs overflow-x-auto">envoy -c bootstrap.yaml</pre>
			</div>
		</div>
	</div>

	<!-- Configuration Preview and Certificate Output -->
	<div class="lg:col-span-2 space-y-6">
		<!-- Generated Certificate Section -->
		{#if generatedCertificate}
			<div class="bg-white rounded-lg shadow-md p-6">
				<div class="flex justify-between items-start mb-4">
					<div>
						<h2 class="text-lg font-semibold text-gray-900">Generated Certificate</h2>
						<p class="text-sm text-gray-500 mt-1">
							SPIFFE URI: <code class="bg-gray-100 px-1 rounded text-xs"
								>{generatedCertificate.spiffeUri}</code
							>
						</p>
					</div>
					<div class="text-right">
						<span
							class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {getDaysUntilExpiry(
								generatedCertificate.expiresAt
							) > 30
								? 'bg-green-100 text-green-800'
								: 'bg-amber-100 text-amber-800'}"
						>
							Expires: {formatExpiryDate(generatedCertificate.expiresAt)}
						</span>
					</div>
				</div>

				<!-- Security Warning -->
				<div class="mb-4 bg-red-50 border-l-4 border-red-500 rounded-md p-3">
					<p class="text-red-800 text-sm font-medium">Security Notice</p>
					<p class="text-red-700 text-xs mt-1">
						The private key is only shown once. Save it securely now. It cannot be retrieved later.
					</p>
				</div>

				<!-- Certificate Files -->
				<div class="space-y-4">
					<!-- Certificate -->
					<div>
						<div class="flex justify-between items-center mb-2">
							<label class="block text-sm font-medium text-gray-700">
								Client Certificate
								<code class="ml-2 text-xs text-gray-500 bg-gray-100 px-1 rounded">{certPath}</code>
							</label>
							<button
								onclick={() => copyCertificateField('certificate')}
								class="text-xs text-blue-600 hover:text-blue-800"
							>
								{certCopySuccess['certificate'] ? 'Copied!' : 'Copy'}
							</button>
						</div>
						<textarea
							readonly
							value={generatedCertificate.certificate}
							class="w-full h-32 px-3 py-2 text-xs font-mono bg-gray-900 text-green-400 border border-gray-300 rounded-md resize-none"
						></textarea>
					</div>

					<!-- Private Key -->
					<div>
						<div class="flex justify-between items-center mb-2">
							<label class="block text-sm font-medium text-gray-700">
								Private Key
								<code class="ml-2 text-xs text-gray-500 bg-gray-100 px-1 rounded">{keyPath}</code>
								<span class="ml-2 text-xs text-red-600">(Save securely!)</span>
							</label>
							<button
								onclick={() => copyCertificateField('privateKey')}
								class="text-xs text-blue-600 hover:text-blue-800"
							>
								{certCopySuccess['privateKey'] ? 'Copied!' : 'Copy'}
							</button>
						</div>
						<textarea
							readonly
							value={generatedCertificate.privateKey}
							class="w-full h-32 px-3 py-2 text-xs font-mono bg-gray-900 text-yellow-400 border border-gray-300 rounded-md resize-none"
						></textarea>
					</div>

					<!-- CA Chain -->
					<div>
						<div class="flex justify-between items-center mb-2">
							<label class="block text-sm font-medium text-gray-700">
								CA Certificate Chain
								<code class="ml-2 text-xs text-gray-500 bg-gray-100 px-1 rounded">{caPath}</code>
							</label>
							<button
								onclick={() => copyCertificateField('caChain')}
								class="text-xs text-blue-600 hover:text-blue-800"
							>
								{certCopySuccess['caChain'] ? 'Copied!' : 'Copy'}
							</button>
						</div>
						<textarea
							readonly
							value={generatedCertificate.caChain}
							class="w-full h-32 px-3 py-2 text-xs font-mono bg-gray-900 text-blue-400 border border-gray-300 rounded-md resize-none"
						></textarea>
					</div>
				</div>
			</div>
		{/if}

		<!-- Configuration Preview -->
		<div class="bg-white rounded-lg shadow-md p-6">
			<div class="flex justify-between items-center mb-4">
				<h2 class="text-lg font-semibold text-gray-900">Configuration Preview</h2>
				<span class="text-sm text-gray-500"> bootstrap-{currentTeam}.{format} </span>
			</div>

			{#if isLoading}
				<div class="flex justify-center items-center py-12">
					<div class="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-600"></div>
				</div>
			{:else if bootstrapConfig}
				<div class="relative">
					<pre
						class="bg-gray-900 text-gray-100 rounded-lg p-4 overflow-x-auto text-sm"><code
							class="hljs">{@html highlightedCode}</code></pre>
				</div>
			{:else}
				<p class="text-center text-gray-500 py-12">
					Select a team to generate bootstrap configuration
				</p>
			{/if}
		</div>
	</div>
</div>
