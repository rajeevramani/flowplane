<script lang="ts">
	import type { CompressorConfig } from '$lib/api/types';
	import { CompressorConfigSchema } from '$lib/schemas/filter-configs';
	import { Info, Plus, Trash2, Settings, ChevronRight } from 'lucide-svelte';

	interface Props {
		config: CompressorConfig;
		onConfigChange: (config: CompressorConfig) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Content settings
	let minContentLength = $state<number>(
		config.response_direction_config?.common_config?.min_content_length ?? 30
	);
	let contentTypes = $state<string[]>(
		config.response_direction_config?.common_config?.content_type ?? [
			'application/json',
			'application/xml',
			'text/plain',
			'text/html',
			'text/css',
			'application/javascript'
		]
	);
	let disableOnEtagHeader = $state(
		config.response_direction_config?.common_config?.disable_on_etag_header ?? false
	);
	let removeAcceptEncodingHeader = $state(
		config.response_direction_config?.common_config?.remove_accept_encoding_header ?? false
	);

	// Compressor library settings
	let compressionLevel = $state<string>(
		config.compressor_library?.compression_level ?? 'best_speed'
	);
	let compressionStrategy = $state<string>(
		config.compressor_library?.compression_strategy ?? 'default_strategy'
	);
	let memoryLevel = $state<number>(config.compressor_library?.memory_level ?? 5);
	let windowBits = $state<number>(config.compressor_library?.window_bits ?? 12);
	let chunkSize = $state<number>(config.compressor_library?.chunk_size ?? 4096);

	// Advanced
	let showAdvanced = $state(false);

	// New content type input
	let newContentType = $state('');

	// Validation errors
	let validationErrors = $state<string[]>([]);

	const COMPRESSION_LEVELS = [
		{ value: 'best_speed', label: 'Best Speed' },
		{ value: 'best_compression', label: 'Best Compression' },
		{ value: 'default_compression', label: 'Default' }
	];

	const COMPRESSION_STRATEGIES = [
		{ value: 'default_strategy', label: 'Default' },
		{ value: 'filtered', label: 'Filtered' },
		{ value: 'huffman_only', label: 'Huffman Only' },
		{ value: 'rle', label: 'RLE' },
		{ value: 'fixed', label: 'Fixed' }
	];

	function updateParent() {
		const cfg: CompressorConfig = {
			response_direction_config: {
				common_config: {
					min_content_length: minContentLength,
					content_type: contentTypes.length > 0 ? contentTypes : undefined,
					disable_on_etag_header: disableOnEtagHeader || undefined,
					remove_accept_encoding_header: removeAcceptEncodingHeader || undefined
				}
			},
			compressor_library: {
				type: 'gzip',
				compression_level: compressionLevel as 'best_speed' | 'best_compression' | 'default_compression',
				compression_strategy: compressionStrategy as 'default_strategy' | 'filtered' | 'huffman_only' | 'rle' | 'fixed',
				memory_level: memoryLevel,
				window_bits: windowBits,
				chunk_size: chunkSize
			}
		};

		const result = CompressorConfigSchema.safeParse(cfg);
		validationErrors = result.success
			? []
			: result.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`);

		onConfigChange(cfg);
	}

	function addContentType() {
		const trimmed = newContentType.trim();
		if (trimmed && !contentTypes.includes(trimmed)) {
			contentTypes = [...contentTypes, trimmed];
			newContentType = '';
			updateParent();
		}
	}

	function removeContentType(ct: string) {
		contentTypes = contentTypes.filter((c) => c !== ct);
		updateParent();
	}

	function handleKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			addContentType();
		}
	}
</script>

<div class="space-y-6">
	<!-- Info Box -->
	<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
		<div class="flex items-start gap-2">
			<Info class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0" />
			<div class="text-xs text-blue-700">
				<p class="font-medium">Compression (gzip)</p>
				<p class="mt-1">
					Compresses HTTP responses to reduce bandwidth usage. Only responses matching
					the configured content types and exceeding the minimum content length will be compressed.
				</p>
			</div>
		</div>
	</div>

	<!-- Validation Errors -->
	{#if validationErrors.length > 0}
		<div class="rounded-lg border border-red-200 bg-red-50 p-3">
			<ul class="text-xs text-red-700 list-disc list-inside space-y-0.5">
				{#each validationErrors as err}
					<li>{err}</li>
				{/each}
			</ul>
		</div>
	{/if}

	<!-- Content Settings -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Content Settings</h3>
		<div class="space-y-4">
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Minimum Content Length (bytes)
				</label>
				<input
					type="number"
					bind:value={minContentLength}
					oninput={updateParent}
					min="0"
					class="w-40 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				/>
				<p class="text-xs text-gray-500 mt-1">
					Responses smaller than this will not be compressed (default: 30 bytes)
				</p>
			</div>

			<div>
				<h4 class="text-sm font-medium text-gray-700 mb-2">Content Types to Compress</h4>
				<div class="flex items-center gap-2 mb-2">
					<input
						type="text"
						bind:value={newContentType}
						onkeydown={handleKeydown}
						placeholder="e.g., application/json"
						class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<button
						type="button"
						onclick={addContentType}
						class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
					>
						<Plus class="w-4 h-4" />
					</button>
				</div>
				{#if contentTypes.length > 0}
					<div class="flex flex-wrap gap-1.5">
						{#each contentTypes as ct}
							<span class="inline-flex items-center gap-1 px-2 py-1 bg-gray-100 text-gray-700 text-xs rounded-md">
								{ct}
								<button
									type="button"
									onclick={() => removeContentType(ct)}
									class="text-gray-400 hover:text-red-500"
								>
									<Trash2 class="w-3 h-3" />
								</button>
							</span>
						{/each}
					</div>
				{/if}
				<p class="text-xs text-gray-500 mt-2">
					Leave empty to compress all content types (press Enter to add)
				</p>
			</div>
		</div>
	</div>

	<!-- Compression Algorithm -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Compression Algorithm</h3>
		<div class="space-y-4">
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Compression Level
				</label>
				<select
					bind:value={compressionLevel}
					onchange={updateParent}
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				>
					{#each COMPRESSION_LEVELS as level}
						<option value={level.value}>{level.label}</option>
					{/each}
				</select>
				<p class="text-xs text-gray-500 mt-1">
					Trade-off between compression ratio and CPU usage
				</p>
			</div>

			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">
					Compression Strategy
				</label>
				<select
					bind:value={compressionStrategy}
					onchange={updateParent}
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				>
					{#each COMPRESSION_STRATEGIES as strategy}
						<option value={strategy.value}>{strategy.label}</option>
					{/each}
				</select>
			</div>
		</div>
	</div>

	<!-- Advanced Gzip Settings -->
	<div>
		<button
			type="button"
			onclick={() => (showAdvanced = !showAdvanced)}
			class="flex items-center gap-2 text-sm font-medium text-gray-600 hover:text-gray-900"
		>
			<Settings class="w-4 h-4" />
			<ChevronRight class="w-4 h-4 transition-transform {showAdvanced ? 'rotate-90' : ''}" />
			Advanced Gzip Settings
		</button>

		{#if showAdvanced}
			<div class="mt-4 space-y-4 pl-6 border-l-2 border-gray-200">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Memory Level (1-9)
					</label>
					<input
						type="number"
						bind:value={memoryLevel}
						oninput={updateParent}
						min="1"
						max="9"
						class="w-24 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Higher values use more memory but compress faster
					</p>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Window Bits (9-15)
					</label>
					<input
						type="number"
						bind:value={windowBits}
						oninput={updateParent}
						min="9"
						max="15"
						class="w-24 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Higher values give better compression at the cost of memory
					</p>
				</div>

				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Chunk Size (bytes)
					</label>
					<input
						type="number"
						bind:value={chunkSize}
						oninput={updateParent}
						min="1024"
						class="w-32 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="text-xs text-gray-500 mt-1">
						Internal compression buffer size (default: 4096)
					</p>
				</div>

				<!-- Behavior -->
				<div class="pt-2 border-t border-gray-200">
					<h4 class="text-sm font-medium text-gray-700 mb-3">Behavior</h4>
					<div class="space-y-3">
						<label class="flex items-center gap-3">
							<input
								type="checkbox"
								bind:checked={disableOnEtagHeader}
								onchange={updateParent}
								class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
							/>
							<div>
								<span class="text-sm font-medium text-gray-700">Disable on ETag Header</span>
								<p class="text-xs text-gray-500">Skip compression for responses with ETag headers</p>
							</div>
						</label>

						<label class="flex items-center gap-3">
							<input
								type="checkbox"
								bind:checked={removeAcceptEncodingHeader}
								onchange={updateParent}
								class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
							/>
							<div>
								<span class="text-sm font-medium text-gray-700">Remove Accept-Encoding Header</span>
								<p class="text-xs text-gray-500">Remove the Accept-Encoding header after making the compression decision</p>
							</div>
						</label>
					</div>
				</div>
			</div>
		{/if}
	</div>
</div>
