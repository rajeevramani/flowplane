<script lang="ts">
	import { page } from '$app/stores';
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import {
		ArrowLeft,
		Edit,
		Download,
		Trash2,
		Copy,
		CheckCircle,
		Server,
		Shield,
		Puzzle,
		AlertTriangle
	} from 'lucide-svelte';
	import type { CustomWasmFilterResponse, FilterResponse } from '$lib/api/types';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import DeleteConfirmModal from '$lib/components/DeleteConfirmModal.svelte';

	let currentTeam = $state<string>('');
	let filterId = $derived($page.params.id);

	// Data
	let customFilter = $state<CustomWasmFilterResponse | null>(null);
	let filterInstances = $state<FilterResponse[]>([]);
	let isLoading = $state(true);
	let error = $state<string | null>(null);

	// Delete modal state
	let showDeleteModal = $state(false);
	let isDeleting = $state(false);

	// Copy state
	let copiedField = $state<string | null>(null);

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
		await loadData();
	});

	async function loadData() {
		if (!currentTeam || !filterId) return;

		isLoading = true;
		error = null;

		try {
			// Load custom filter details
			customFilter = await apiClient.getCustomWasmFilter(currentTeam, filterId);

			// Load filter instances to find usage
			const filters = await apiClient.listFilters();
			filterInstances = filters.filter((f) => f.filterType === customFilter?.filter_type);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load custom filter';
			console.error('Failed to load custom filter:', e);
		} finally {
			isLoading = false;
		}
	}

	// Format bytes
	function formatBytes(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	// Format date
	function formatDate(dateStr: string): string {
		const date = new Date(dateStr);
		return date.toLocaleDateString('en-US', {
			year: 'numeric',
			month: 'long',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	// Copy to clipboard
	async function copyToClipboard(text: string, field: string) {
		try {
			await navigator.clipboard.writeText(text);
			copiedField = field;
			setTimeout(() => {
				copiedField = null;
			}, 2000);
		} catch (e) {
			console.error('Failed to copy:', e);
		}
	}

	// Download binary
	async function handleDownload() {
		if (!customFilter) return;

		try {
			const blob = await apiClient.downloadCustomWasmFilterBinary(currentTeam, customFilter.id);
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `${customFilter.name}.wasm`;
			document.body.appendChild(a);
			a.click();
			document.body.removeChild(a);
			URL.revokeObjectURL(url);
		} catch (e) {
			console.error('Failed to download:', e);
		}
	}

	// Open delete modal
	function openDeleteModal() {
		showDeleteModal = true;
	}

	// Confirm delete
	async function confirmDelete() {
		if (!customFilter) return;

		isDeleting = true;
		try {
			await apiClient.deleteCustomWasmFilter(currentTeam, customFilter.id);
			goto('/custom-filters');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to delete custom filter';
		} finally {
			isDeleting = false;
			showDeleteModal = false;
		}
	}

	// Cancel delete
	function cancelDelete() {
		showDeleteModal = false;
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8">
	<!-- Header -->
	<div class="flex items-center justify-between mb-6">
		<div class="flex items-center gap-4">
			<a
				href="/custom-filters"
				class="text-blue-600 hover:text-blue-800"
				aria-label="Back to custom filters"
			>
				<ArrowLeft class="h-6 w-6" />
			</a>
			<div>
				<h1 class="text-2xl font-bold text-gray-900">
					{customFilter?.display_name ?? 'Custom Filter'}
				</h1>
				{#if customFilter}
					<p class="text-sm text-gray-500 font-mono">{customFilter.name}</p>
				{/if}
			</div>
		</div>

		{#if customFilter}
			<div class="flex gap-2">
				<Button variant="ghost" onclick={() => goto(`/custom-filters/${filterId}/edit`)}>
					<Edit class="h-4 w-4 mr-2" />
					Edit
				</Button>
				<Button variant="ghost" onclick={handleDownload}>
					<Download class="h-4 w-4 mr-2" />
					Download
				</Button>
				<Button variant="danger" onclick={openDeleteModal}>
					<Trash2 class="h-4 w-4 mr-2" />
					Delete
				</Button>
			</div>
		{/if}
	</div>

	{#if isLoading}
		<div class="flex items-center justify-center py-12">
			<div class="flex flex-col items-center gap-3">
				<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
				<span class="text-sm text-gray-600">Loading custom filter...</span>
			</div>
		</div>
	{:else if error}
		<div class="bg-red-50 border border-red-200 rounded-md p-4">
			<div class="flex items-center gap-2">
				<AlertTriangle class="h-5 w-5 text-red-500" />
				<p class="text-sm text-red-800">{error}</p>
			</div>
		</div>
	{:else if customFilter}
		<div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
			<!-- Left Column -->
			<div class="space-y-6">
				<!-- Details Card -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Details</h2>
					<dl class="space-y-3">
						<div>
							<dt class="text-sm font-medium text-gray-500">Display Name</dt>
							<dd class="text-sm text-gray-900">{customFilter.display_name}</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Description</dt>
							<dd class="text-sm text-gray-900">{customFilter.description || '-'}</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Team</dt>
							<dd class="text-sm text-gray-900">{customFilter.team}</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Created</dt>
							<dd class="text-sm text-gray-900">
								{formatDate(customFilter.created_at)}
								{#if customFilter.created_by}
									<span class="text-gray-500">by {customFilter.created_by}</span>
								{/if}
							</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Last Updated</dt>
							<dd class="text-sm text-gray-900">{formatDate(customFilter.updated_at)}</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Version</dt>
							<dd class="text-sm text-gray-900">{customFilter.version}</dd>
						</div>
					</dl>
				</div>

				<!-- Technical Info Card -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Technical Info</h2>
					<dl class="space-y-3">
						<div>
							<dt class="text-sm font-medium text-gray-500">Filter Type</dt>
							<dd class="flex items-center gap-2">
								<code class="text-sm bg-gray-100 px-2 py-1 rounded font-mono">
									{customFilter.filter_type}
								</code>
								<button
									onclick={() => copyToClipboard(customFilter!.filter_type, 'filterType')}
									class="p-1 text-gray-400 hover:text-gray-600"
									title="Copy filter type"
								>
									{#if copiedField === 'filterType'}
										<CheckCircle class="h-4 w-4 text-green-600" />
									{:else}
										<Copy class="h-4 w-4" />
									{/if}
								</button>
							</dd>
							<p class="mt-1 text-xs text-gray-500">Use this type when creating filter instances</p>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">SHA256 Hash</dt>
							<dd class="flex items-center gap-2">
								<code class="text-xs bg-gray-100 px-2 py-1 rounded font-mono truncate max-w-xs">
									{customFilter.wasm_sha256}
								</code>
								<button
									onclick={() => copyToClipboard(customFilter!.wasm_sha256, 'sha256')}
									class="p-1 text-gray-400 hover:text-gray-600"
									title="Copy SHA256"
								>
									{#if copiedField === 'sha256'}
										<CheckCircle class="h-4 w-4 text-green-600" />
									{:else}
										<Copy class="h-4 w-4" />
									{/if}
								</button>
							</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Binary Size</dt>
							<dd class="text-sm text-gray-900">{formatBytes(customFilter.wasm_size_bytes)}</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Runtime</dt>
							<dd class="flex items-center gap-2">
								<Server class="h-4 w-4 text-gray-400" />
								<span class="text-sm text-gray-900">{customFilter.runtime}</span>
							</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Failure Policy</dt>
							<dd class="flex items-center gap-2">
								<Shield class="h-4 w-4 text-gray-400" />
								<Badge
									variant={customFilter.failure_policy === 'FAIL_CLOSED' ? 'orange' : 'blue'}
								>
									{customFilter.failure_policy}
								</Badge>
							</dd>
						</div>
						<div>
							<dt class="text-sm font-medium text-gray-500">Attachment Points</dt>
							<dd class="flex gap-2 mt-1">
								{#each customFilter.attachment_points || [] as point}
									<Badge variant="gray">{point}</Badge>
								{/each}
							</dd>
						</div>
					</dl>
				</div>
			</div>

			<!-- Right Column -->
			<div class="space-y-6">
				<!-- Config Schema Card -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<div class="flex items-center justify-between mb-4">
						<h2 class="text-lg font-semibold text-gray-900">Configuration Schema</h2>
						<button
							onclick={() =>
								copyToClipboard(JSON.stringify(customFilter!.config_schema, null, 2), 'configSchema')}
							class="p-1 text-gray-400 hover:text-gray-600"
							title="Copy schema"
						>
							{#if copiedField === 'configSchema'}
								<CheckCircle class="h-4 w-4 text-green-600" />
							{:else}
								<Copy class="h-4 w-4" />
							{/if}
						</button>
					</div>
					<pre
						class="bg-gray-900 text-gray-100 rounded-md p-4 overflow-auto max-h-64 text-xs"><code>{JSON.stringify(customFilter.config_schema, null, 2)}</code></pre>
				</div>

				<!-- Per-Route Schema Card -->
				{#if customFilter.per_route_config_schema}
					<div class="bg-white rounded-lg shadow-md p-6">
						<div class="flex items-center justify-between mb-4">
							<h2 class="text-lg font-semibold text-gray-900">Per-Route Config Schema</h2>
							<button
								onclick={() =>
									copyToClipboard(
										JSON.stringify(customFilter!.per_route_config_schema, null, 2),
										'perRouteSchema'
									)}
								class="p-1 text-gray-400 hover:text-gray-600"
								title="Copy schema"
							>
								{#if copiedField === 'perRouteSchema'}
									<CheckCircle class="h-4 w-4 text-green-600" />
								{:else}
									<Copy class="h-4 w-4" />
								{/if}
							</button>
						</div>
						<pre
							class="bg-gray-900 text-gray-100 rounded-md p-4 overflow-auto max-h-48 text-xs"><code>{JSON.stringify(customFilter.per_route_config_schema, null, 2)}</code></pre>
					</div>
				{/if}

				<!-- UI Hints Card -->
				{#if customFilter.ui_hints && Object.keys(customFilter.ui_hints).length > 0}
					<div class="bg-white rounded-lg shadow-md p-6">
						<h2 class="text-lg font-semibold text-gray-900 mb-4">UI Hints</h2>
						<pre
							class="bg-gray-900 text-gray-100 rounded-md p-4 overflow-auto max-h-32 text-xs"><code>{JSON.stringify(customFilter.ui_hints, null, 2)}</code></pre>
					</div>
				{/if}

				<!-- Usage Card -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<div class="flex items-center gap-2 mb-4">
						<Puzzle class="h-5 w-5 text-gray-500" />
						<h2 class="text-lg font-semibold text-gray-900">Usage</h2>
					</div>

					{#if filterInstances.length === 0}
						<div class="text-center py-6">
							<div class="text-gray-400 mb-2">
								<Puzzle class="h-8 w-8 mx-auto" />
							</div>
							<p class="text-sm text-gray-600">
								No filter instances are using this custom filter yet.
							</p>
							<p class="text-xs text-gray-500 mt-1">
								Create a filter with type <code class="bg-gray-100 px-1 rounded"
									>{customFilter.filter_type}</code
								> to use it.
							</p>
						</div>
					{:else}
						<div class="space-y-2">
							{#each filterInstances as instance}
								<a
									href={`/filters/${instance.id}`}
									class="block p-3 bg-gray-50 rounded-lg hover:bg-gray-100 transition-colors"
								>
									<div class="flex items-center justify-between">
										<div>
											<p class="text-sm font-medium text-gray-900">{instance.name}</p>
											<p class="text-xs text-gray-500">
												Type: {instance.filterType}
											</p>
										</div>
										{#if instance.attachmentCount !== undefined}
											<Badge variant={instance.attachmentCount > 0 ? 'green' : 'gray'}>
												{instance.attachmentCount} {instance.attachmentCount === 1 ? 'attachment' : 'attachments'}
											</Badge>
										{/if}
									</div>
								</a>
							{/each}
						</div>
						<p class="mt-3 text-xs text-gray-500 text-center">
							{filterInstances.length}
							{filterInstances.length === 1 ? 'instance' : 'instances'} using this filter
						</p>
					{/if}
				</div>
			</div>
		</div>
	{/if}
</div>

<!-- Delete Confirmation Modal -->
{#if customFilter}
	<DeleteConfirmModal
		show={showDeleteModal}
		resourceType="custom filter"
		resourceName={customFilter.display_name}
		onConfirm={confirmDelete}
		onCancel={cancelDelete}
		loading={isDeleting}
		warningMessage={filterInstances.length > 0
			? `Warning: This filter is used by ${filterInstances.length} filter instance(s). Deleting may break those configurations.`
			: undefined}
	/>
{/if}
