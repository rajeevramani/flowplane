<script lang="ts">
	import { Copy, Edit2, X, Check } from 'lucide-svelte';

	interface Props {
		jsonString: string;
		editable?: boolean;
		onJsonChange?: (json: string) => void;
	}

	let { jsonString, editable = false, onJsonChange }: Props = $props();

	let isEditing = $state(false);
	let editedJson = $state('');
	let copySuccess = $state(false);
	let parseError = $state<string | null>(null);

	// Handle copy to clipboard
	async function handleCopy() {
		try {
			await navigator.clipboard.writeText(jsonString);
			copySuccess = true;
			setTimeout(() => {
				copySuccess = false;
			}, 2000);
		} catch (err) {
			console.error('Failed to copy:', err);
		}
	}

	// Enter edit mode
	function handleEdit() {
		isEditing = true;
		editedJson = jsonString;
		parseError = null;
	}

	// Apply changes
	function handleApply() {
		try {
			// Validate JSON
			JSON.parse(editedJson);
			parseError = null;

			// Call the onChange callback
			onJsonChange?.(editedJson);
			isEditing = false;
		} catch (e) {
			parseError = e instanceof Error ? e.message : 'Invalid JSON';
		}
	}

	// Cancel editing
	function handleCancel() {
		isEditing = false;
		editedJson = '';
		parseError = null;
	}
</script>

<div class="bg-gray-900 text-gray-100 h-full flex flex-col">
	<!-- Header -->
	<div class="flex items-center justify-between p-4 border-b border-gray-800">
		<h2 class="text-lg font-semibold">JSON {isEditing ? 'Editor' : 'Preview'}</h2>
		<div class="flex gap-2">
			{#if !isEditing}
				<!-- View Mode Buttons -->
				<button
					onclick={handleCopy}
					class="px-3 py-1.5 text-sm bg-gray-800 hover:bg-gray-700 rounded-md flex items-center gap-2 transition-colors"
					title="Copy to clipboard"
				>
					{#if copySuccess}
						<Check class="w-4 h-4 text-green-400" />
						<span class="text-green-400">Copied!</span>
					{:else}
						<Copy class="w-4 h-4" />
						Copy
					{/if}
				</button>
				{#if editable && onJsonChange}
					<button
						onclick={handleEdit}
						class="px-3 py-1.5 text-sm bg-blue-600 hover:bg-blue-500 rounded-md flex items-center gap-2 transition-colors"
						title="Edit JSON"
					>
						<Edit2 class="w-4 h-4" />
						Edit
					</button>
				{/if}
			{:else}
				<!-- Edit Mode Buttons -->
				<button
					onclick={handleCancel}
					class="px-3 py-1.5 text-sm bg-gray-800 hover:bg-gray-700 rounded-md flex items-center gap-2 transition-colors"
				>
					<X class="w-4 h-4" />
					Cancel
				</button>
				<button
					onclick={handleApply}
					class="px-3 py-1.5 text-sm bg-green-600 hover:bg-green-500 rounded-md flex items-center gap-2 transition-colors"
				>
					<Check class="w-4 h-4" />
					Apply
				</button>
			{/if}
		</div>
	</div>

	<!-- Content -->
	<div class="flex-1 overflow-auto p-4">
		{#if isEditing}
			<!-- Edit Mode -->
			<textarea
				bind:value={editedJson}
				class="w-full h-full bg-gray-800 text-gray-100 font-mono text-sm p-4 rounded-md border border-gray-700 focus:outline-none focus:ring-2 focus:ring-blue-500 resize-none"
				spellcheck="false"
			></textarea>
			{#if parseError}
				<div class="mt-2 p-3 bg-red-900 bg-opacity-30 border border-red-700 rounded-md">
					<p class="text-sm text-red-300">
						<strong>Parse Error:</strong> {parseError}
					</p>
				</div>
			{/if}
		{:else}
			<!-- View Mode -->
			<pre
				class="text-sm font-mono overflow-x-auto"><code>{jsonString}</code></pre>
		{/if}
	</div>

	<!-- Footer Info -->
	{#if !isEditing}
		<div class="p-4 border-t border-gray-800 text-xs text-gray-400">
			<p>
				This JSON will be sent to the backend API.
				{#if editable && onJsonChange}
					Click "Edit" to modify directly and sync back to the form.
				{:else}
					Changes in the form automatically update this JSON.
				{/if}
			</p>
		</div>
	{/if}
</div>
