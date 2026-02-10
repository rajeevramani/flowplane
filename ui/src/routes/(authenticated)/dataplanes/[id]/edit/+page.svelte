<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { ArrowLeft, Save, Server, Download, Copy, Check } from 'lucide-svelte';
	import type { DataplaneResponse, MtlsStatusResponse, GenerateCertificateResponse } from '$lib/api/types';
	import Button from '$lib/components/Button.svelte';
	import { validateRequired, validateIdentifier, runValidators } from '$lib/utils/validators';
	import { selectedTeam } from '$lib/stores/team';
	import hljs from 'highlight.js/lib/core';
	import yaml from 'highlight.js/lib/languages/yaml';
	import json from 'highlight.js/lib/languages/json';
	import 'highlight.js/styles/github-dark.css';

	// Register highlight.js languages
	hljs.registerLanguage('yaml', yaml);
	hljs.registerLanguage('json', json);

	// Get dataplane name from URL (the [id] param is actually the name)
	let dataplaneName = $derived($page.params.id);

	// Get current team from store
	let currentTeam = $state<string>('');
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Form state
	let formState = $state({
		name: '',
		gatewayHost: '',
		description: ''
	});

	let originalDataplane = $state<DataplaneResponse | null>(null);
	let isLoading = $state(true);
	let isSubmitting = $state(false);
	let error = $state<string | null>(null);
	let envoyConfig = $state<string | null>(null);
	let highlightedCode = $state<string>('');
	let showEnvoyConfig = $state(false);
	let copied = $state(false);
	let format = $state<'yaml' | 'json'>('yaml');

	// mTLS state
	let mtlsStatus = $state<MtlsStatusResponse | null>(null);
	let enableMtlsInEnvoyConfig = $state(false);

	// Default certificate paths
	const DEFAULT_CERT_PATH = '/etc/envoy/certs/client.pem';
	const DEFAULT_KEY_PATH = '/etc/envoy/certs/client-key.pem';
	const DEFAULT_CA_PATH = '/etc/envoy/certs/ca.pem';

	let certPath = $state(DEFAULT_CERT_PATH);
	let keyPath = $state(DEFAULT_KEY_PATH);
	let caPath = $state(DEFAULT_CA_PATH);

	// Certificate generation state
	let proxyId = $state('');
	let proxyIdError = $state<string | null>(null);
	let isGeneratingCert = $state(false);
	let generatedCertificate = $state<GenerateCertificateResponse | null>(null);
	let certCopySuccess = $state<Record<string, boolean>>({});

	// Proxy ID validation regex
	const PROXY_ID_REGEX = /^[a-zA-Z0-9][a-zA-Z0-9_-]*$/;

	// Load dataplane and mTLS status on mount
	onMount(async () => {
		await Promise.all([loadDataplane(), loadMtlsStatus()]);
	});

	// Load mTLS status
	async function loadMtlsStatus() {
		try {
			mtlsStatus = await apiClient.getMtlsStatus();
		} catch (err) {
			// If we can't get mTLS status, assume it's not configured
			mtlsStatus = null;
		}
	}

	async function loadDataplane() {
		if (!dataplaneName || !currentTeam) {
			error = 'No dataplane name or team provided';
			return;
		}

		isLoading = true;
		error = null;

		try {
			const dataplane = await apiClient.getDataplane(currentTeam, dataplaneName);
			originalDataplane = dataplane;

			formState = {
				name: dataplane.name,
				gatewayHost: dataplane.gatewayHost || '',
				description: dataplane.description || ''
			};
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load dataplane';
		} finally {
			isLoading = false;
		}
	}

	// Validation
	function validateForm(): string | null {
		return runValidators([
			() => validateRequired(formState.name, 'Dataplane name'),
			() => validateIdentifier(formState.name, 'Dataplane name')
		]);
	}

	// Handle form submission
	async function handleSubmit(e: Event) {
		e.preventDefault();

		if (!dataplaneName || !currentTeam) {
			error = 'No dataplane name or team provided';
			return;
		}

		const validationError = validateForm();
		if (validationError) {
			error = validationError;
			return;
		}

		isSubmitting = true;
		error = null;

		try {
			await apiClient.updateDataplane(currentTeam, dataplaneName, {
				gatewayHost: formState.gatewayHost || undefined,
				description: formState.description || undefined
			});

			goto('/dataplanes');
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to update dataplane';
		} finally {
			isSubmitting = false;
		}
	}

	// Navigate back
	function handleBack() {
		goto('/dataplanes');
	}

	// Build envoy config options
	function getEnvoyConfigOptions() {
		return {
			format,
			mtls: enableMtlsInEnvoyConfig && mtlsStatus?.pkiMountConfigured ? true : undefined,
			certPath: enableMtlsInEnvoyConfig ? certPath : undefined,
			keyPath: enableMtlsInEnvoyConfig ? keyPath : undefined,
			caPath: enableMtlsInEnvoyConfig ? caPath : undefined
		};
	}

	// Download envoy config
	async function handleDownloadEnvoyConfig() {
		if (!dataplaneName || !currentTeam) return;

		try {
			const config = await apiClient.getDataplaneEnvoyConfig(currentTeam, dataplaneName, getEnvoyConfigOptions());
			const mimeType = format === 'yaml' ? 'application/yaml' : 'application/json';
			const blob = new Blob([config], { type: mimeType });
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `envoy-config-${formState.name}.${format}`;
			document.body.appendChild(a);
			a.click();
			document.body.removeChild(a);
			URL.revokeObjectURL(url);
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to download envoy config';
		}
	}

	// Load envoy config with current options
	async function loadEnvoyConfig() {
		if (!dataplaneName || !currentTeam) return;

		try {
			envoyConfig = await apiClient.getDataplaneEnvoyConfig(currentTeam, dataplaneName, getEnvoyConfigOptions());
			highlightedCode = hljs.highlight(envoyConfig, { language: format }).value;
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to load envoy config';
		}
	}

	// Show/hide envoy config preview
	async function toggleEnvoyConfigPreview() {
		if (!dataplaneName || !currentTeam) return;

		if (!showEnvoyConfig && !envoyConfig) {
			await loadEnvoyConfig();
			if (error) return;
		}
		showEnvoyConfig = !showEnvoyConfig;
	}

	// Handle format change - reload envoy config if visible
	async function handleFormatChange() {
		if (!showEnvoyConfig) return;
		await loadEnvoyConfig();
	}

	// Handle mTLS toggle - reload envoy config if visible
	async function handleMtlsToggle() {
		if (!showEnvoyConfig) return;
		await loadEnvoyConfig();
	}

	// Handle certificate path change - reload envoy config if visible
	async function handlePathChange() {
		if (!showEnvoyConfig || !enableMtlsInEnvoyConfig) return;
		await loadEnvoyConfig();
	}

	// Validate proxy ID
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

	// Handle proxy ID input
	function handleProxyIdInput(event: Event) {
		const target = event.target as HTMLInputElement;
		proxyId = target.value;
		proxyIdError = validateProxyId(proxyId);
	}

	// Generate certificate
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

			// Enable mTLS in envoy config after generating certificate
			enableMtlsInEnvoyConfig = true;
			if (showEnvoyConfig) {
				await loadEnvoyConfig();
			}
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to generate certificate';
		} finally {
			isGeneratingCert = false;
		}
	}

	// Copy certificate field to clipboard
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

	// Format expiry date
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

	// Get days until expiry
	function getDaysUntilExpiry(dateString: string): number {
		const expiry = new Date(dateString);
		const now = new Date();
		const diffMs = expiry.getTime() - now.getTime();
		return Math.ceil(diffMs / (1000 * 60 * 60 * 24));
	}

	// Copy envoy config to clipboard
	async function handleCopyEnvoyConfig() {
		if (envoyConfig) {
			try {
				await navigator.clipboard.writeText(envoyConfig);
				copied = true;
				setTimeout(() => {
					copied = false;
				}, 2000);
			} catch (err) {
				console.error('Failed to copy:', err);
			}
		}
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<button
			onclick={handleBack}
			class="flex items-center text-sm text-gray-600 hover:text-gray-900 mb-4"
		>
			<ArrowLeft class="h-4 w-4 mr-1" />
			Back to Dataplanes
		</button>

		<div class="flex items-center justify-between">
			<div class="flex items-center gap-3">
				<div class="p-3 bg-blue-100 rounded-lg">
					<Server class="h-6 w-6 text-blue-600" />
				</div>
				<div>
					<h1 class="text-2xl font-bold text-gray-900">Edit Dataplane</h1>
					<p class="text-sm text-gray-600">
						{#if originalDataplane}
							Editing <span class="font-medium">{originalDataplane.name}</span>
						{:else}
							Loading...
						{/if}
					</p>
				</div>
			</div>

			{#if originalDataplane}
				<Button onclick={handleDownloadEnvoyConfig} variant="secondary">
					<Download class="h-4 w-4 mr-2" />
					Download Envoy Config
				</Button>
			{/if}
		</div>
	</div>

	<!-- Loading State -->
	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading dataplane...</span>
			</div>
		</div>
	{:else if error && !originalDataplane}
		<!-- Error State (failed to load) -->
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else}
		<!-- Error Alert (form errors) -->
		{#if error}
			<div class="bg-red-50 border border-red-200 rounded-md p-4 mb-6">
				<p class="text-sm text-red-800">{error}</p>
			</div>
		{/if}

		<!-- Form -->
		<form onsubmit={handleSubmit} class="space-y-6">
			<!-- Basic Information -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<h2 class="text-lg font-medium text-gray-900 mb-4">Basic Information</h2>

				<div class="space-y-4">
					<!-- Name -->
					<div>
						<label for="name" class="block text-sm font-medium text-gray-700 mb-1">
							Name <span class="text-red-500">*</span>
						</label>
						<input
							type="text"
							id="name"
							bind:value={formState.name}
							placeholder="e.g., prod-gateway, staging-envoy"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							required
						/>
						<p class="mt-1 text-xs text-gray-500">
							A unique identifier for this dataplane. Use lowercase letters, numbers, and hyphens.
						</p>
					</div>

					<!-- Description -->
					<div>
						<label for="description" class="block text-sm font-medium text-gray-700 mb-1">
							Description
						</label>
						<textarea
							id="description"
							bind:value={formState.description}
							placeholder="Optional description for this dataplane"
							rows="2"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						></textarea>
					</div>
				</div>
			</div>

			<!-- Network Configuration -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<h2 class="text-lg font-medium text-gray-900 mb-4">Network Configuration</h2>

				<div class="space-y-4">
					<!-- Gateway Host -->
					<div>
						<label for="gatewayHost" class="block text-sm font-medium text-gray-700 mb-1">
							Gateway Host
						</label>
						<input
							type="text"
							id="gatewayHost"
							bind:value={formState.gatewayHost}
							placeholder="e.g., 10.0.0.5, host.docker.internal, envoy.service.consul"
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<p class="mt-1 text-xs text-gray-500">
							The address where this Envoy instance is reachable from the Control Plane. Leave
							empty to use localhost (127.0.0.1).
						</p>
					</div>
				</div>
			</div>

			<!-- Envoy Config Preview -->
			<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
				<div class="flex items-center justify-between mb-4">
					<h2 class="text-lg font-medium text-gray-900">Envoy Configuration</h2>
					<button
						type="button"
						onclick={toggleEnvoyConfigPreview}
						class="text-sm text-blue-600 hover:text-blue-800"
					>
						{showEnvoyConfig ? 'Hide' : 'Show'} Envoy Config
					</button>
				</div>

				<!-- Format Selection -->
				<div class="mb-4">
					<label class="block text-sm font-medium text-gray-700 mb-2">Format</label>
					<div class="flex gap-4">
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
					<div class="border-t border-gray-200 pt-4 mb-4">
						<h3 class="text-sm font-medium text-gray-900 mb-3">mTLS Configuration</h3>

						{#if mtlsStatus.pkiMountConfigured}
							<!-- mTLS is available -->
							<div class="space-y-3">
								<label class="flex items-center">
									<input
										type="checkbox"
										bind:checked={enableMtlsInEnvoyConfig}
										onchange={handleMtlsToggle}
										class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
									/>
									<span class="ml-2 text-sm text-gray-700">Enable mTLS in envoy config</span>
								</label>

								{#if enableMtlsInEnvoyConfig}
									<div class="ml-6 space-y-2">
										<div>
											<label class="block text-xs font-medium text-gray-600 mb-1">Certificate Path</label>
											<input
												type="text"
												bind:value={certPath}
												onblur={handlePathChange}
												class="w-full px-2 py-1 text-sm border border-gray-300 rounded-md focus:ring-blue-500 focus:border-blue-500"
											/>
										</div>
										<div>
											<label class="block text-xs font-medium text-gray-600 mb-1">Key Path</label>
											<input
												type="text"
												bind:value={keyPath}
												onblur={handlePathChange}
												class="w-full px-2 py-1 text-sm border border-gray-300 rounded-md focus:ring-blue-500 focus:border-blue-500"
											/>
										</div>
										<div>
											<label class="block text-xs font-medium text-gray-600 mb-1">CA Path</label>
											<input
												type="text"
												bind:value={caPath}
												onblur={handlePathChange}
												class="w-full px-2 py-1 text-sm border border-gray-300 rounded-md focus:ring-blue-500 focus:border-blue-500"
											/>
										</div>
									</div>
								{/if}

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
										placeholder="e.g., {formState.name}-proxy"
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
										type="button"
										onclick={generateCertificate}
										disabled={isGeneratingCert || !currentTeam || !proxyId || !!proxyIdError}
										class="mt-3 w-full px-4 py-2 text-sm font-medium text-white bg-green-600 rounded-md hover:bg-green-700 disabled:opacity-50 disabled:cursor-not-allowed"
									>
										{isGeneratingCert ? 'Generating...' : 'Generate Certificate'}
									</button>
								</div>
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

				{#if showEnvoyConfig && envoyConfig}
					<div class="relative">
						<button
							type="button"
							onclick={handleCopyEnvoyConfig}
							class="absolute top-2 right-2 p-2 text-gray-400 hover:text-gray-600 bg-gray-800 rounded z-10"
							title="Copy to clipboard"
						>
							{#if copied}
								<Check class="h-4 w-4 text-green-400" />
							{:else}
								<Copy class="h-4 w-4" />
							{/if}
						</button>
						<pre class="bg-gray-900 rounded-md p-4 text-sm overflow-x-auto max-h-96"><code class="hljs">{@html highlightedCode}</code></pre>
					</div>
					<p class="mt-2 text-xs text-gray-500">
						Use this configuration to start your Envoy instance with the correct node ID and xDS
						connection settings.
					</p>
				{:else}
					<p class="text-sm text-gray-600">
						Click "Show Envoy Config" to preview the Envoy configuration for this
						dataplane in {format.toUpperCase()} format.
					</p>
				{/if}
			</div>

			<!-- Generated Certificate Section -->
			{#if generatedCertificate}
				<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
					<div class="flex justify-between items-start mb-4">
						<div>
							<h2 class="text-lg font-medium text-gray-900">Generated Certificate</h2>
							<p class="text-sm text-gray-500 mt-1">
								SPIFFE URI: <code class="bg-gray-100 px-1 rounded text-xs">{generatedCertificate.spiffeUri}</code>
							</p>
						</div>
						<div class="text-right">
							<span
								class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {getDaysUntilExpiry(generatedCertificate.expiresAt) > 30
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
									type="button"
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
									type="button"
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
									type="button"
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

			<!-- Usage Instructions -->
			<div class="bg-blue-50 border-l-4 border-blue-500 rounded-md p-4">
				<h3 class="text-sm font-medium text-blue-800 mb-2">How to Use</h3>
				<div class="text-sm text-blue-700 space-y-2">
					{#if generatedCertificate}
						<p>1. Save the certificate files to your proxy at the configured paths</p>
						<p>2. Download or copy the Envoy configuration</p>
						<p>3. Save it as <code class="bg-blue-100 px-1 rounded">envoy-config.yaml</code></p>
						<p>4. Start Envoy with the configuration:</p>
					{:else}
						<p>1. Download or copy the Envoy configuration</p>
						<p>2. Save it as <code class="bg-blue-100 px-1 rounded">envoy-config.{format}</code></p>
						<p>3. Start Envoy with the configuration:</p>
					{/if}
					<pre class="mt-2 p-2 bg-blue-100 rounded text-xs overflow-x-auto">envoy -c envoy-config.{format}</pre>
				</div>
			</div>

			<!-- Form Actions -->
			<div class="flex justify-end gap-3">
				<Button onclick={handleBack} variant="secondary" disabled={isSubmitting}>
					Cancel
				</Button>
				<Button type="submit" variant="primary" disabled={isSubmitting}>
					{#if isSubmitting}
						<div class="animate-spin rounded-full h-4 w-4 border-b-2 border-white mr-2"></div>
						Saving...
					{:else}
						<Save class="h-4 w-4 mr-2" />
						Save Changes
					{/if}
				</Button>
			</div>
		</form>
	{/if}
</div>
