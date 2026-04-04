<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { onMount } from 'svelte';
	import { Shield, ShieldAlert, ShieldCheck, ShieldX, Search, XCircle } from 'lucide-svelte';
	import type { CertificateMetadata, ListCertificatesResponse, SessionInfoResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import { adminSummary, adminSummaryLoading, adminSummaryError, getAdminSummary } from '$lib/stores/adminSummary';
	import AdminResourceSummary from '$lib/components/AdminResourceSummary.svelte';
	import Badge from '$lib/components/Badge.svelte';

	let sessionInfo = $state<SessionInfoResponse | null>(null);

	let isLoading = $state(true);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);
	let searchQuery = $state('');
	let statusFilter = $state<'all' | 'valid' | 'expired' | 'revoked'>('all');
	let currentTeam = $state<string>('');

	// Data
	let certificates = $state<CertificateMetadata[]>([]);
	let total = $state(0);

	// Revoke modal state
	let showRevokeModal = $state(false);
	let revokeTarget = $state<CertificateMetadata | null>(null);
	let revokeReason = $state('');
	let isRevoking = $state(false);
	let revokeError = $state<string | null>(null);

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
			const response: ListCertificatesResponse = await apiClient.listProxyCertificates(currentTeam);
			certificates = response.certificates;
			total = response.total;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load certificates';
			console.error('Failed to load certificates:', e);
		} finally {
			isLoading = false;
		}
	}

	// Stats
	let stats = $derived({
		total: certificates.length,
		valid: certificates.filter(c => c.isValid).length,
		expired: certificates.filter(c => c.isExpired).length,
		revoked: certificates.filter(c => c.isRevoked).length
	});

	// Filtered certificates
	let filteredCertificates = $derived(
		certificates.filter(cert => {
			if (statusFilter === 'valid' && !cert.isValid) return false;
			if (statusFilter === 'expired' && !cert.isExpired) return false;
			if (statusFilter === 'revoked' && !cert.isRevoked) return false;
			if (searchQuery) {
				const query = searchQuery.toLowerCase();
				return (
					cert.proxyId.toLowerCase().includes(query) ||
					cert.serialNumber.toLowerCase().includes(query) ||
					cert.spiffeUri.toLowerCase().includes(query) ||
					cert.id.toLowerCase().includes(query)
				);
			}
			return true;
		})
	);

	function getStatusBadge(cert: CertificateMetadata): { variant: 'green' | 'red' | 'yellow'; label: string } {
		if (cert.isRevoked) return { variant: 'red', label: 'Revoked' };
		if (cert.isExpired) return { variant: 'yellow', label: 'Expired' };
		if (cert.isValid) return { variant: 'green', label: 'Valid' };
		return { variant: 'yellow', label: 'Unknown' };
	}

	function formatDate(dateStr: string): string {
		const date = new Date(dateStr);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function isExpiringSoon(cert: CertificateMetadata): boolean {
		if (cert.isExpired || cert.isRevoked) return false;
		const expiresAt = new Date(cert.expiresAt);
		const now = new Date();
		const sevenDays = new Date(now.getTime() + 7 * 24 * 60 * 60 * 1000);
		return expiresAt <= sevenDays && expiresAt > now;
	}

	function openRevokeModal(cert: CertificateMetadata) {
		revokeTarget = cert;
		revokeReason = '';
		revokeError = null;
		showRevokeModal = true;
	}

	function closeRevokeModal() {
		showRevokeModal = false;
		revokeTarget = null;
		revokeReason = '';
		revokeError = null;
	}

	async function handleRevoke() {
		if (!revokeTarget || !revokeReason.trim()) return;

		isRevoking = true;
		revokeError = null;

		try {
			await apiClient.revokeProxyCertificate(currentTeam, revokeTarget.id, revokeReason.trim());
			closeRevokeModal();
			await loadData();
		} catch (err) {
			revokeError = err instanceof Error ? err.message : 'Failed to revoke certificate';
		} finally {
			isRevoking = false;
		}
	}

	function truncateSerial(serial: string): string {
		if (serial.length <= 16) return serial;
		return serial.slice(0, 8) + '...' + serial.slice(-8);
	}
</script>

{#if sessionInfo?.isPlatformAdmin}
<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Proxy Certificates</h1>
		<p class="mt-2 text-sm text-gray-600">Platform-wide proxy certificate summary across all organizations and teams.</p>
	</div>
	{#if $adminSummaryLoading}
		<div class="flex items-center justify-center py-12"><div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div></div>
	{:else if $adminSummaryError}
		<div class="bg-red-50 border border-red-200 rounded-md p-4"><p class="text-sm text-red-800">{$adminSummaryError}</p></div>
	{:else if $adminSummary}
		<AdminResourceSummary summary={$adminSummary} highlightResource="proxy-certificates" />
	{/if}
</div>
{:else}
<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="mb-8">
		<h1 class="text-3xl font-bold text-gray-900">Proxy Certificates</h1>
		<p class="mt-2 text-sm text-gray-600">
			Manage mTLS proxy certificates for the <span class="font-medium">{currentTeam}</span> team
		</p>
	</div>

	<!-- Stats Cards -->
	<div class="grid grid-cols-1 md:grid-cols-4 gap-4 mb-6">
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Total Certificates</p>
					<p class="text-2xl font-bold text-gray-900">{stats.total}</p>
				</div>
				<div class="p-3 bg-blue-100 rounded-lg">
					<Shield class="h-6 w-6 text-blue-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Valid</p>
					<p class="text-2xl font-bold text-green-600">{stats.valid}</p>
				</div>
				<div class="p-3 bg-green-100 rounded-lg">
					<ShieldCheck class="h-6 w-6 text-green-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Expired</p>
					<p class="text-2xl font-bold text-yellow-600">{stats.expired}</p>
				</div>
				<div class="p-3 bg-yellow-100 rounded-lg">
					<ShieldAlert class="h-6 w-6 text-yellow-600" />
				</div>
			</div>
		</div>

		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-4">
			<div class="flex items-center justify-between">
				<div>
					<p class="text-sm font-medium text-gray-600">Revoked</p>
					<p class="text-2xl font-bold text-red-600">{stats.revoked}</p>
				</div>
				<div class="p-3 bg-red-100 rounded-lg">
					<ShieldX class="h-6 w-6 text-red-600" />
				</div>
			</div>
		</div>
	</div>

	<!-- Filters -->
	<div class="mb-6 flex flex-wrap gap-4">
		<div class="relative w-full md:w-96">
			<Search class="absolute left-3 top-1/2 transform -translate-y-1/2 h-4 w-4 text-gray-400" />
			<input
				type="text"
				bind:value={searchQuery}
				placeholder="Search by proxy ID, serial number, or SPIFFE URI..."
				class="w-full pl-10 pr-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
			/>
		</div>

		<select
			bind:value={statusFilter}
			class="px-4 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
		>
			<option value="all">All Status</option>
			<option value="valid">Valid</option>
			<option value="expired">Expired</option>
			<option value="revoked">Revoked</option>
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
				<span class="text-sm text-gray-600">Loading certificates...</span>
			</div>
		</div>
	{:else if error}
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{:else if filteredCertificates.length === 0}
		<!-- Empty State -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-12 text-center">
			<Shield class="h-12 w-12 text-gray-400 mx-auto mb-4" />
			<h3 class="text-lg font-medium text-gray-900 mb-2">
				{searchQuery || statusFilter !== 'all' ? 'No certificates found' : 'No proxy certificates yet'}
			</h3>
			<p class="text-sm text-gray-600">
				{searchQuery || statusFilter !== 'all'
					? 'Try adjusting your filters'
					: 'Proxy certificates are generated via the CLI or API when configuring mTLS'}
			</p>
		</div>
	{:else}
		<!-- Table -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 overflow-hidden">
			<table class="min-w-full divide-y divide-gray-200">
				<thead class="bg-gray-50">
					<tr>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Proxy ID
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Status
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Serial Number
						</th>
						<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
							Issued
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
					{#each filteredCertificates as cert}
						{@const status = getStatusBadge(cert)}
						<tr class="hover:bg-gray-50 transition-colors">
							<!-- Proxy ID -->
							<td class="px-6 py-4">
								<div class="flex flex-col">
									<span class="text-sm font-medium text-gray-900">{cert.proxyId}</span>
									<span class="text-xs text-gray-500 font-mono truncate max-w-[200px]" title={cert.spiffeUri}>
										{cert.spiffeUri}
									</span>
								</div>
							</td>

							<!-- Status -->
							<td class="px-6 py-4">
								<Badge variant={status.variant}>
									{status.label}
								</Badge>
								{#if isExpiringSoon(cert)}
									<span class="block text-xs text-amber-600 font-medium mt-1">Expiring soon</span>
								{/if}
								{#if cert.isRevoked && cert.revokedReason}
									<span class="block text-xs text-gray-500 mt-1 truncate max-w-[150px]" title={cert.revokedReason}>
										{cert.revokedReason}
									</span>
								{/if}
							</td>

							<!-- Serial Number -->
							<td class="px-6 py-4">
								<span class="text-sm text-gray-600 font-mono" title={cert.serialNumber}>
									{truncateSerial(cert.serialNumber)}
								</span>
							</td>

							<!-- Issued -->
							<td class="px-6 py-4">
								<span class="text-sm text-gray-600">{formatDate(cert.issuedAt)}</span>
							</td>

							<!-- Expires -->
							<td class="px-6 py-4">
								{#if cert.isExpired}
									<span class="text-sm text-red-600 font-medium">{formatDate(cert.expiresAt)}</span>
								{:else if isExpiringSoon(cert)}
									<span class="text-sm text-amber-600 font-medium">{formatDate(cert.expiresAt)}</span>
								{:else}
									<span class="text-sm text-gray-600">{formatDate(cert.expiresAt)}</span>
								{/if}
							</td>

							<!-- Actions -->
							<td class="px-6 py-4 text-right">
								{#if cert.isValid}
									<button
										onclick={() => openRevokeModal(cert)}
										class="inline-flex items-center gap-1.5 px-3 py-1.5 text-sm text-red-600 hover:bg-red-50 rounded-md transition-colors border border-red-200"
										title="Revoke certificate"
									>
										<XCircle class="h-4 w-4" />
										Revoke
									</button>
								{:else}
									<span class="text-sm text-gray-400">-</span>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		<!-- Pagination info -->
		{#if total > certificates.length}
			<div class="mt-4 flex justify-center">
				<p class="text-sm text-gray-600">Showing {certificates.length} of {total} certificates</p>
			</div>
		{/if}
	{/if}
</div>
{/if}

<!-- Revoke Modal -->
{#if showRevokeModal && revokeTarget}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50"
		onclick={(e) => { if (e.target === e.currentTarget) closeRevokeModal(); }}
	>
		<div class="bg-white rounded-lg shadow-xl p-6 max-w-lg w-full mx-4" onclick={(e) => e.stopPropagation()}>
			<h2 class="text-lg font-semibold text-gray-900 mb-4">Revoke Certificate</h2>

			<div class="space-y-4">
				<div class="bg-red-50 border border-red-200 rounded-md p-3">
					<p class="text-sm text-red-800">
						This action cannot be undone. The certificate for proxy <strong>{revokeTarget.proxyId}</strong> will be permanently revoked.
					</p>
				</div>

				<div class="text-sm text-gray-600 space-y-1">
					<p><span class="font-medium">Serial:</span> <span class="font-mono">{revokeTarget.serialNumber}</span></p>
					<p><span class="font-medium">SPIFFE URI:</span> <span class="font-mono text-xs">{revokeTarget.spiffeUri}</span></p>
				</div>

				<div>
					<label for="revoke-reason" class="block text-sm font-medium text-gray-700 mb-1">
						Reason for revocation <span class="text-red-500">*</span>
					</label>
					<textarea
						id="revoke-reason"
						bind:value={revokeReason}
						placeholder="e.g., Key compromise, certificate rotation, decommissioning proxy..."
						rows="3"
						maxlength="500"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-red-500 text-sm"
					></textarea>
					<p class="text-xs text-gray-500 mt-1">{revokeReason.length}/500 characters</p>
				</div>

				{#if revokeError}
					<div class="bg-red-50 border border-red-200 rounded-md p-3">
						<p class="text-sm text-red-800">{revokeError}</p>
					</div>
				{/if}

				<div class="flex justify-end gap-3 pt-2">
					<button
						onclick={closeRevokeModal}
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50"
						disabled={isRevoking}
					>
						Cancel
					</button>
					<button
						onclick={handleRevoke}
						class="px-4 py-2 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700 disabled:opacity-50 disabled:cursor-not-allowed"
						disabled={isRevoking || !revokeReason.trim()}
					>
						{#if isRevoking}
							Revoking...
						{:else}
							Revoke Certificate
						{/if}
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
