<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { parse as parseYaml } from 'yaml';
	import type { OpenApiSpec } from '$lib/api/types';

	let activeTab = $state<'upload' | 'paste'>('paste');
	let specContent = $state('');
	let fileInput = $state<HTMLInputElement | null>(null);
	let isDragging = $state(false);

	// Parsing state
	let parsedSpec = $state<OpenApiSpec | null>(null);
	let parseError = $state<string | null>(null);

	// Configuration
	let config = $state({
		team: '',
		listenerIsolation: false,
		port: 8080
	});

	// Submission state
	let isSubmitting = $state(false);
	let submitError = $state<string | null>(null);
	let toast = $state<{ message: string; type: 'success' | 'error' } | null>(null);

	onMount(async () => {
		// Check authentication
		try {
			await apiClient.getSessionInfo();
		} catch (error) {
			goto('/login');
		}
	});

	function handleFileSelect(event: Event) {
		const target = event.target as HTMLInputElement;
		if (target.files && target.files[0]) {
			const file = target.files[0];
			readFile(file);
		}
	}

	function handleDragOver(event: DragEvent) {
		event.preventDefault();
		isDragging = true;
	}

	function handleDragLeave() {
		isDragging = false;
	}

	function handleDrop(event: DragEvent) {
		event.preventDefault();
		isDragging = false;

		if (event.dataTransfer?.files && event.dataTransfer.files[0]) {
			const file = event.dataTransfer.files[0];
			readFile(file);
		}
	}

	function readFile(file: File) {
		const reader = new FileReader();
		reader.onload = (e) => {
			if (e.target?.result) {
				specContent = e.target.result as string;
				parseSpec();
			}
		};
		reader.readAsText(file);
	}

	function parseSpec() {
		parseError = null;
		parsedSpec = null;

		if (!specContent.trim()) {
			parseError = 'Spec content is empty';
			return;
		}

		try {
			// Try parsing as JSON first
			try {
				parsedSpec = JSON.parse(specContent);
			} catch {
				// If JSON fails, try YAML
				parsedSpec = parseYaml(specContent);
			}

			// Validate basic structure
			if (!parsedSpec || typeof parsedSpec !== 'object') {
				throw new Error('Invalid spec format');
			}

			if (!parsedSpec.info) {
				throw new Error('Missing required "info" field');
			}

			if (!parsedSpec.paths || typeof parsedSpec.paths !== 'object') {
				throw new Error('Missing or invalid "paths" field');
			}

		} catch (e: any) {
			parseError = `Failed to parse spec: ${e.message}`;
			parsedSpec = null;
		}
	}

	function handlePasteChange() {
		parseSpec();
	}

	async function handleSubmit() {
		if (!parsedSpec) {
			submitError = 'Please provide a valid OpenAPI specification';
			return;
		}

		try {
			isSubmitting = true;
			submitError = null;

			const response = await apiClient.importOpenApiSpec({
				spec: specContent,
				team: config.team || undefined,
				listenerIsolation: config.listenerIsolation,
				port: config.listenerIsolation ? config.port : undefined
			});

			showToast(`OpenAPI spec imported successfully! API Definition ID: ${response.id}`, 'success');

			// Redirect to dashboard after a short delay
			// TODO: Redirect to API definition detail page when that page is implemented
			setTimeout(() => {
				goto('/dashboard');
			}, 2000);

		} catch (e: any) {
			const errorMsg = e.message || 'Failed to import OpenAPI spec';
			submitError = errorMsg;
			showToast(errorMsg, 'error');
		} finally {
			isSubmitting = false;
		}
	}

	function showToast(message: string, type: 'success' | 'error') {
		toast = { message, type };
		setTimeout(() => {
			toast = null;
		}, 5000);
	}

	function getPathCount(): number {
		if (!parsedSpec?.paths) return 0;
		return Object.keys(parsedSpec.paths).length;
	}

	function getMethodsCount(): number {
		if (!parsedSpec?.paths) return 0;
		let count = 0;
		for (const path of Object.values(parsedSpec.paths)) {
			if (typeof path === 'object' && path !== null) {
				for (const key of Object.keys(path)) {
					if (['get', 'post', 'put', 'delete', 'patch', 'options', 'head'].includes(key.toLowerCase())) {
						count++;
					}
				}
			}
		}
		return count;
	}
</script>

<div class="min-h-screen bg-gray-50">
	<!-- Navigation -->
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a href="/dashboard" class="text-blue-600 hover:text-blue-800" aria-label="Back to dashboard">
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M10 19l-7-7m0 0l7-7m-7 7h18"
							/>
						</svg>
					</a>
					<h1 class="text-xl font-bold text-gray-900">Import OpenAPI Specification</h1>
				</div>
			</div>
		</div>
	</nav>

	<main class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
		<div class="grid grid-cols-1 lg:grid-cols-2 gap-8">
			<!-- Left column: Input -->
			<div>
				<!-- Tabs -->
				<div class="bg-white rounded-t-lg shadow-md border-b border-gray-200">
					<div class="flex">
						<button
							onclick={() => activeTab = 'paste'}
							class="flex-1 px-6 py-4 text-sm font-medium transition-colors {activeTab === 'paste'
								? 'text-blue-600 border-b-2 border-blue-600'
								: 'text-gray-600 hover:text-gray-900'}"
						>
							Paste Spec
						</button>
						<button
							onclick={() => activeTab = 'upload'}
							class="flex-1 px-6 py-4 text-sm font-medium transition-colors {activeTab === 'upload'
								? 'text-blue-600 border-b-2 border-blue-600'
								: 'text-gray-600 hover:text-gray-900'}"
						>
							Upload File
						</button>
					</div>
				</div>

				<!-- Tab content -->
				<div class="bg-white rounded-b-lg shadow-md p-6">
					{#if activeTab === 'paste'}
						<!-- Paste textarea -->
						<div class="mb-4">
							<label for="spec-content" class="block text-sm font-medium text-gray-700 mb-2">
								OpenAPI Specification (YAML or JSON)
							</label>
							<textarea
								id="spec-content"
								bind:value={specContent}
								oninput={handlePasteChange}
								class="w-full h-96 px-3 py-2 border border-gray-300 rounded-md font-mono text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
								placeholder="Paste your OpenAPI spec here..."
							></textarea>
						</div>
					{:else}
						<!-- File upload -->
						<div class="mb-4">
							<div class="block text-sm font-medium text-gray-700 mb-2">
								Upload OpenAPI File
							</div>
							<div
								role="button"
								tabindex="0"
								class="border-2 border-dashed rounded-lg p-8 text-center transition-colors {isDragging
									? 'border-blue-500 bg-blue-50'
									: 'border-gray-300 hover:border-gray-400'}"
								ondragover={handleDragOver}
								ondragleave={handleDragLeave}
								ondrop={handleDrop}
							>
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
										d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12"
									/>
								</svg>
								<p class="mt-2 text-sm text-gray-600">Drag and drop your OpenAPI file here, or</p>
								<button
									type="button"
									onclick={() => fileInput?.click()}
									class="mt-2 px-4 py-2 text-sm font-medium text-blue-600 hover:text-blue-800"
								>
									Browse Files
								</button>
								<input
									type="file"
									bind:this={fileInput}
									onchange={handleFileSelect}
									accept=".yaml,.yml,.json"
									class="hidden"
								/>
							</div>
						</div>
					{/if}

					<!-- Parse error -->
					{#if parseError}
						<div class="mt-4 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
							<p class="text-red-800 text-sm">{parseError}</p>
						</div>
					{/if}
				</div>
			</div>

			<!-- Right column: Preview & Configuration -->
			<div class="space-y-6">
				<!-- Spec Preview -->
				{#if parsedSpec}
					<div class="bg-white rounded-lg shadow-md p-6">
						<h2 class="text-lg font-semibold text-gray-900 mb-4">Spec Preview</h2>

						<div class="space-y-4">
							<div>
								<h3 class="text-sm font-medium text-gray-700">API Information</h3>
								<div class="mt-2 bg-gray-50 rounded-md p-3 space-y-1">
									<p class="text-sm"><span class="font-medium">Title:</span> {parsedSpec.info.title}</p>
									<p class="text-sm"><span class="font-medium">Version:</span> {parsedSpec.info.version}</p>
									{#if parsedSpec.info.description}
										<p class="text-sm"><span class="font-medium">Description:</span> {parsedSpec.info.description}</p>
									{/if}
								</div>
							</div>

							{#if parsedSpec.servers && parsedSpec.servers.length > 0}
								<div>
									<h3 class="text-sm font-medium text-gray-700">Servers</h3>
									<div class="mt-2 bg-gray-50 rounded-md p-3 space-y-1">
										{#each parsedSpec.servers as server}
											<p class="text-sm font-mono">{server.url}</p>
										{/each}
									</div>
								</div>
							{/if}

							<div>
								<h3 class="text-sm font-medium text-gray-700">Paths</h3>
								<div class="mt-2 bg-gray-50 rounded-md p-3">
									<p class="text-sm">
										<span class="font-medium">{getPathCount()}</span> paths with
										<span class="font-medium">{getMethodsCount()}</span> operations
									</p>
								</div>
							</div>
						</div>
					</div>
				{/if}

				<!-- Configuration Form -->
				<div class="bg-white rounded-lg shadow-md p-6">
					<h2 class="text-lg font-semibold text-gray-900 mb-4">Import Configuration</h2>

					<form onsubmit={(e) => { e.preventDefault(); handleSubmit(); }} class="space-y-4">
						<!-- Team -->
						<div>
							<label for="team" class="block text-sm font-medium text-gray-700 mb-2">
								Team (Optional)
							</label>
							<input
								id="team"
								type="text"
								bind:value={config.team}
								class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								placeholder="Leave empty for auto-detection"
							/>
							<p class="mt-1 text-xs text-gray-500">
								If not specified, team will be extracted from the first server URL domain
							</p>
						</div>

						<!-- Listener Isolation -->
						<div>
							<label class="flex items-center">
								<input
									type="checkbox"
									bind:checked={config.listenerIsolation}
									class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
								/>
								<span class="ml-2 text-sm text-gray-700">Enable Listener Isolation</span>
							</label>
							<p class="mt-1 text-xs text-gray-500 ml-6">
								Create a dedicated listener for this API (default: shared listener)
							</p>
						</div>

						<!-- Port (only if listener isolation is enabled) -->
						{#if config.listenerIsolation}
							<div>
								<label for="port" class="block text-sm font-medium text-gray-700 mb-2">
									Listener Port
								</label>
								<input
									id="port"
									type="number"
									bind:value={config.port}
									min="1024"
									max="65535"
									class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
							</div>
						{/if}

						<!-- Submit error -->
						{#if submitError}
							<div class="bg-red-50 border-l-4 border-red-500 rounded-md p-4">
								<p class="text-red-800 text-sm">{submitError}</p>
							</div>
						{/if}

						<!-- Submit button -->
						<button
							type="submit"
							disabled={!parsedSpec || isSubmitting}
							class="w-full px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
						>
							{isSubmitting ? 'Importing...' : 'Import OpenAPI Spec'}
						</button>
					</form>
				</div>
			</div>
		</div>
	</main>

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
