<script lang="ts">
	import type { ListenerTlsContextInput } from '$lib/api/types';
	import { Lock, AlertTriangle } from 'lucide-svelte';

	interface Props {
		tlsContext: ListenerTlsContextInput | null;
		onTlsContextChange: (context: ListenerTlsContextInput | null) => void;
		compact?: boolean;
	}

	let { tlsContext, onTlsContextChange, compact = false }: Props = $props();

	// State
	let enabled = $state(tlsContext !== null);
	let certChainFile = $state(tlsContext?.certChainFile || '');
	let privateKeyFile = $state(tlsContext?.privateKeyFile || '');
	let caCertFile = $state(tlsContext?.caCertFile || '');
	let requireClientCertificate = $state(tlsContext?.requireClientCertificate || false);
	let minTlsVersion = $state<'V1_0' | 'V1_1' | 'V1_2' | 'V1_3'>(tlsContext?.minTlsVersion || 'V1_2');
	let errors = $state<Record<string, string>>({});

	// Sync state from props
	$effect(() => {
		enabled = tlsContext !== null;
		if (tlsContext) {
			certChainFile = tlsContext.certChainFile || '';
			privateKeyFile = tlsContext.privateKeyFile || '';
			caCertFile = tlsContext.caCertFile || '';
			requireClientCertificate = tlsContext.requireClientCertificate || false;
			minTlsVersion = tlsContext.minTlsVersion || 'V1_2';
		}
	});

	function toggleEnabled() {
		enabled = !enabled;
		if (!enabled) {
			onTlsContextChange(null);
			errors = {};
		} else {
			propagateChanges();
		}
	}

	function handleCertChainChange(e: Event) {
		const target = e.target as HTMLInputElement;
		certChainFile = target.value;
		propagateChanges();
	}

	function handlePrivateKeyChange(e: Event) {
		const target = e.target as HTMLInputElement;
		privateKeyFile = target.value;
		propagateChanges();
	}

	function handleCaCertChange(e: Event) {
		const target = e.target as HTMLInputElement;
		caCertFile = target.value;
		propagateChanges();
	}

	function handleRequireClientCertChange(e: Event) {
		const target = e.target as HTMLInputElement;
		requireClientCertificate = target.checked;
		propagateChanges();
	}

	function handleTlsVersionChange(e: Event) {
		const target = e.target as HTMLSelectElement;
		minTlsVersion = target.value as 'V1_0' | 'V1_1' | 'V1_2' | 'V1_3';
		propagateChanges();
	}

	function validate(): Record<string, string> {
		const errs: Record<string, string> = {};

		if (!certChainFile.trim()) {
			errs.certChainFile = 'Certificate chain file is required when TLS is enabled';
		} else if (certChainFile.length > 500) {
			errs.certChainFile = 'Path must be 500 characters or less';
		}

		if (!privateKeyFile.trim()) {
			errs.privateKeyFile = 'Private key file is required when TLS is enabled';
		} else if (privateKeyFile.length > 500) {
			errs.privateKeyFile = 'Path must be 500 characters or less';
		}

		if (caCertFile && caCertFile.length > 500) {
			errs.caCertFile = 'Path must be 500 characters or less';
		}

		if (requireClientCertificate && !caCertFile.trim()) {
			errs.caCertFile = 'CA certificate is required when client certificate is required';
		}

		return errs;
	}

	function propagateChanges() {
		if (!enabled) {
			onTlsContextChange(null);
			errors = {};
			return;
		}

		errors = validate();

		const context: ListenerTlsContextInput = {
			certChainFile: certChainFile.trim() || undefined,
			privateKeyFile: privateKeyFile.trim() || undefined,
			caCertFile: caCertFile.trim() || undefined,
			requireClientCertificate,
			minTlsVersion
		};

		onTlsContextChange(context);
	}

	// Show deprecation warning for old TLS versions
	let showDeprecationWarning = $derived(enabled && (minTlsVersion === 'V1_0' || minTlsVersion === 'V1_1'));
</script>

<div class="space-y-4">
	<!-- Enable/Disable Toggle -->
	<label class="flex items-center gap-3 cursor-pointer">
		<input
			type="checkbox"
			checked={enabled}
			onchange={toggleEnabled}
			class="h-4 w-4 text-blue-600 focus:ring-blue-500 rounded"
		/>
		<span class="text-sm font-medium text-gray-700 flex items-center gap-2">
			{#if enabled}
				<Lock class="h-4 w-4 text-green-600" />
			{/if}
			Enable TLS
		</span>
	</label>

	{#if !compact}
		<p class="text-xs text-gray-500 ml-7">
			When enabled, this filter chain will use HTTPS with the configured certificates.
		</p>
	{/if}

	<!-- TLS Configuration Form (only shown when enabled) -->
	{#if enabled}
		<div class="ml-7 space-y-4 border-l-2 border-blue-200 pl-4">
			<!-- Certificate Chain File -->
			<div>
				<label for="cert-chain-file" class="block text-sm font-medium text-gray-700 mb-1">
					Certificate Chain File *
				</label>
				<input
					id="cert-chain-file"
					type="text"
					placeholder="/path/to/cert.pem"
					value={certChainFile}
					oninput={handleCertChainChange}
					class="w-full rounded-md border px-3 py-2 text-sm font-mono focus:ring-1 focus:ring-blue-500 {errors.certChainFile ? 'border-red-500' : 'border-gray-300'}"
				/>
				{#if errors.certChainFile}
					<p class="mt-1 text-xs text-red-600">{errors.certChainFile}</p>
				{/if}
			</div>

			<!-- Private Key File -->
			<div>
				<label for="private-key-file" class="block text-sm font-medium text-gray-700 mb-1">
					Private Key File *
				</label>
				<input
					id="private-key-file"
					type="text"
					placeholder="/path/to/key.pem"
					value={privateKeyFile}
					oninput={handlePrivateKeyChange}
					class="w-full rounded-md border px-3 py-2 text-sm font-mono focus:ring-1 focus:ring-blue-500 {errors.privateKeyFile ? 'border-red-500' : 'border-gray-300'}"
				/>
				{#if errors.privateKeyFile}
					<p class="mt-1 text-xs text-red-600">{errors.privateKeyFile}</p>
				{/if}
			</div>

			<!-- Minimum TLS Version -->
			<div>
				<label for="min-tls-version" class="block text-sm font-medium text-gray-700 mb-1">
					Minimum TLS Version
				</label>
				<select
					id="min-tls-version"
					value={minTlsVersion}
					onchange={handleTlsVersionChange}
					class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:ring-1 focus:ring-blue-500"
				>
					<option value="V1_0">TLS 1.0 (deprecated)</option>
					<option value="V1_1">TLS 1.1 (deprecated)</option>
					<option value="V1_2">TLS 1.2</option>
					<option value="V1_3">TLS 1.3 (recommended)</option>
				</select>
				<p class="mt-1 text-xs text-gray-500">Recommended: TLS 1.3</p>
			</div>

			{#if showDeprecationWarning}
				<div class="flex items-start gap-2 p-3 bg-yellow-50 border border-yellow-200 rounded-md">
					<AlertTriangle class="h-4 w-4 text-yellow-600 flex-shrink-0 mt-0.5" />
					<p class="text-xs text-yellow-800">
						TLS {minTlsVersion.replace('V1_', '1.')} is deprecated and should not be used in production. Consider using TLS 1.2 or 1.3.
					</p>
				</div>
			{/if}

			<!-- Mutual TLS Section -->
			<div class="pt-4 border-t border-gray-200">
				{#if !compact}
					<h4 class="text-sm font-medium text-gray-700 mb-3">Mutual TLS (Optional)</h4>
				{/if}

			<!-- CA Certificate File -->
			<div>
				<label for="ca-cert-file" class="block text-sm font-medium text-gray-700 mb-1">
					CA Certificate File {#if !compact}<span class="text-gray-500 font-normal">(optional)</span>{/if}
				</label>
				<input
					id="ca-cert-file"
					type="text"
					placeholder="/path/to/ca.pem"
					value={caCertFile}
					oninput={handleCaCertChange}
					class="w-full rounded-md border px-3 py-2 text-sm font-mono focus:ring-1 focus:ring-blue-500 {errors.caCertFile ? 'border-red-500' : 'border-gray-300'}"
				/>
				{#if errors.caCertFile}
					<p class="mt-1 text-xs text-red-600">{errors.caCertFile}</p>
				{/if}
			</div>

			<!-- Require Client Certificate -->
			<label class="flex items-start gap-3 cursor-pointer">
				<input
					type="checkbox"
					checked={requireClientCertificate}
					onchange={handleRequireClientCertChange}
					class="h-4 w-4 text-blue-600 focus:ring-blue-500 rounded mt-0.5"
				/>
				<div class="flex-1">
					<span class="text-sm font-medium text-gray-700">Require Client Certificate</span>
					{#if !compact}
						<p class="text-xs text-gray-500 mt-1">
							Clients must present valid certificates signed by the CA above.
						</p>
					{/if}
				</div>
			</label>
			</div>

			<!-- Security Warning -->
			{#if !compact}
				<div class="flex items-start gap-2 p-3 bg-blue-50 border border-blue-200 rounded-md">
					<Lock class="h-4 w-4 text-blue-600 flex-shrink-0 mt-0.5" />
					<p class="text-xs text-blue-800">
						Certificate files must be readable by the Envoy proxy. Use absolute paths or paths relative to Envoy's working directory.
					</p>
				</div>
			{/if}
		</div>
	{/if}
</div>
