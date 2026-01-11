<script lang="ts">
	import { X, Save, AlertCircle, Lock } from 'lucide-svelte';
	import Button from '../Button.svelte';
	import Badge from '../Badge.svelte';
	import type { McpTool, McpToolCategory } from '$lib/api/types';

	interface Props {
		show: boolean;
		tool: McpTool | null;
		onClose: () => void;
		onSave: (tool: McpTool) => void;
	}

	let { show, tool, onClose, onSave }: Props = $props();

	// Only editable metadata fields - route definition fields are read-only
	interface FormData {
		id: string;
		name: string;
		description: string | null;
		inputSchema: Record<string, unknown>;
		outputSchema: Record<string, unknown> | null;
	}

	let formData = $state<FormData | null>(null);
	let requestSchemaText = $state('');
	let responseSchemaText = $state('');
	let errors = $state<{ requestSchema?: string; responseSchema?: string }>({});

	// Format the route display for read-only display
	let routeDisplay = $derived(
		tool
			? tool.httpMethod
				? `${tool.httpMethod} ${tool.httpPath || '/'}`
				: tool.httpPath || '/'
			: ''
	);

	// Category display name
	let categoryDisplay = $derived(
		tool?.category === 'control_plane'
			? 'Control Plane'
			: tool?.category === 'gateway_api'
				? 'Gateway API'
				: tool?.category || 'Unknown'
	);

	// Initialize form data when tool changes - only editable fields
	$effect(() => {
		if (tool) {
			formData = {
				id: tool.id,
				name: tool.name,
				description: tool.description,
				inputSchema: tool.inputSchema,
				outputSchema: tool.outputSchema
			};
			requestSchemaText = tool.inputSchema
				? JSON.stringify(tool.inputSchema, null, 2)
				: '';
			responseSchemaText = tool.outputSchema
				? JSON.stringify(tool.outputSchema, null, 2)
				: '';
			errors = {};
		}
	});

	function handleBackdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget) {
			onClose();
		}
	}

	function handleKeyDown(event: KeyboardEvent) {
		if (event.key === 'Escape') {
			onClose();
		}
	}

	function handleSave() {
		if (!formData || !tool) return;

		const newErrors: { requestSchema?: string; responseSchema?: string } = {};
		let inputSchema: Record<string, unknown> = formData.inputSchema;
		let outputSchema: Record<string, unknown> | null = formData.outputSchema;

		// Validate request schema JSON
		if (requestSchemaText.trim()) {
			try {
				inputSchema = JSON.parse(requestSchemaText);
			} catch (e) {
				newErrors.requestSchema = 'Invalid JSON format';
			}
		}

		// Validate response schema JSON
		if (responseSchemaText.trim()) {
			try {
				outputSchema = JSON.parse(responseSchemaText);
			} catch (e) {
				newErrors.responseSchema = 'Invalid JSON format';
			}
		} else {
			outputSchema = null;
		}

		if (Object.keys(newErrors).length > 0) {
			errors = newErrors;
			return;
		}

		// Only update editable metadata fields - preserve route definition fields from original tool
		onSave({
			...tool,
			name: formData.name,
			description: formData.description,
			inputSchema,
			outputSchema
			// Note: httpMethod, httpPath, and category are preserved from original tool (read-only)
		});
		onClose();
	}

	function updateFormField<K extends keyof FormData>(field: K, value: FormData[K]) {
		if (!formData) return;
		formData = { ...formData, [field]: value };
	}
</script>

{#if show && tool && formData}
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4"
		role="dialog"
		aria-modal="true"
		tabindex="-1"
		onclick={handleBackdropClick}
		onkeydown={handleKeyDown}
	>
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="bg-white rounded-lg shadow-xl max-w-4xl w-full max-h-[90vh] overflow-y-auto"
			role="document"
			onclick={(e) => e.stopPropagation()}
		>
			<!-- Header -->
			<div
				class="sticky top-0 bg-white border-b border-gray-200 px-6 py-4 flex items-center justify-between"
			>
				<h2 class="text-xl font-semibold text-gray-900">Edit Tool Information</h2>
				<button
					type="button"
					onclick={onClose}
					class="text-gray-500 hover:text-gray-700 transition-colors"
					aria-label="Close modal"
				>
					<X class="w-6 h-6" />
				</button>
			</div>

			<!-- Content -->
			<div class="px-6 py-6 space-y-6">
				<!-- Read-only Route Definition Section -->
				<div class="bg-gray-50 rounded-lg p-4 border border-gray-200">
					<div class="flex items-center gap-2 mb-3">
						<Lock class="w-4 h-4 text-gray-400" />
						<span class="text-xs font-medium text-gray-500 uppercase tracking-wide">Route Definition (Read-only)</span>
					</div>
					<div class="grid grid-cols-2 gap-4">
						<div>
							<span class="text-xs text-gray-500">Route</span>
							<code class="block mt-1 text-sm font-mono text-gray-900 bg-white px-2 py-1 rounded border border-gray-200">
								{routeDisplay}
							</code>
						</div>
						<div>
							<span class="text-xs text-gray-500">Category</span>
							<div class="mt-1">
								<Badge variant={tool?.category === 'control_plane' ? 'purple' : 'blue'}>
									{categoryDisplay}
								</Badge>
							</div>
						</div>
					</div>
				</div>

				<!-- Editable Metadata Section -->
				<div class="border-t border-gray-200 pt-6">
					<h3 class="text-sm font-medium text-gray-900 mb-4">Editable Metadata</h3>

					<!-- Tool Name -->
					<div class="mb-4">
						<label for="tool-name" class="block text-sm font-medium text-gray-700 mb-2">
							Tool Name
						</label>
						<input
							id="tool-name"
							type="text"
							value={formData.name}
							oninput={(e) => updateFormField('name', e.currentTarget.value)}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
					</div>

					<!-- Description -->
					<div>
						<label for="tool-description" class="block text-sm font-medium text-gray-700 mb-2">
							Description
						</label>
						<textarea
							id="tool-description"
							value={formData.description || ''}
							oninput={(e) => updateFormField('description', e.currentTarget.value || null)}
							rows={3}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
							placeholder="Enter a description for this tool..."
						></textarea>
					</div>
				</div>

				<!-- Request Schema -->
				<div>
					<label for="request-schema" class="block text-sm font-medium text-gray-700 mb-2">
						Request Schema (JSON)
					</label>
					<textarea
						id="request-schema"
						bind:value={requestSchemaText}
						oninput={() => {
							errors = { ...errors, requestSchema: undefined };
						}}
						rows={8}
						class="w-full px-3 py-2 border {errors.requestSchema
							? 'border-red-500'
							: 'border-gray-300'} rounded-md bg-gray-900 text-gray-100 font-mono text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
						placeholder={`{\n  "type": "object",\n  "properties": {\n    "param1": { "type": "string" }\n  }\n}`}
					></textarea>
					{#if errors.requestSchema}
						<div class="mt-1 flex items-center gap-1 text-red-600 text-sm">
							<AlertCircle class="w-4 h-4" />
							{errors.requestSchema}
						</div>
					{/if}
				</div>

				<!-- Response Schema -->
				<div>
					<label for="response-schema" class="block text-sm font-medium text-gray-700 mb-2">
						Response Schema (JSON)
					</label>
					<textarea
						id="response-schema"
						bind:value={responseSchemaText}
						oninput={() => {
							errors = { ...errors, responseSchema: undefined };
						}}
						rows={8}
						class="w-full px-3 py-2 border {errors.responseSchema
							? 'border-red-500'
							: 'border-gray-300'} rounded-md bg-gray-900 text-gray-100 font-mono text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
						placeholder={`{\n  "type": "object",\n  "properties": {\n    "result": { "type": "string" }\n  }\n}`}
					></textarea>
					{#if errors.responseSchema}
						<div class="mt-1 flex items-center gap-1 text-red-600 text-sm">
							<AlertCircle class="w-4 h-4" />
							{errors.responseSchema}
						</div>
					{/if}
				</div>
			</div>

			<!-- Footer -->
			<div
				class="sticky bottom-0 bg-gray-50 border-t border-gray-200 px-6 py-4 flex justify-end gap-3"
			>
				<Button variant="ghost" onclick={onClose}>Cancel</Button>
				<Button variant="primary" onclick={handleSave}>
					<Save class="w-4 h-4 mr-2" />
					Save Changes
				</Button>
			</div>
		</div>
	</div>
{/if}
