<script lang="ts">
	import { X, AlertTriangle, CheckCircle2, ArrowRight, Sparkles } from 'lucide-svelte';
	import Button from '../Button.svelte';
	import Badge from '../Badge.svelte';
	import type { McpTool, LearnedSchemaInfo } from '$lib/api/types';

	interface Props {
		show: boolean;
		tool: McpTool | null;
		learnedSchema: LearnedSchemaInfo | null;
		currentSource: 'openapi' | 'learned' | 'manual';
		requiresForce: boolean;
		onConfirm: (force: boolean) => void;
		onCancel: () => void;
	}

	let { show, tool, learnedSchema, currentSource, requiresForce, onConfirm, onCancel }: Props = $props();

	let forceCheckbox = $state(false);

	// Reset checkbox when modal shows/hides
	$effect(() => {
		if (show) {
			forceCheckbox = false;
		}
	});

	function handleBackdropClick(event: MouseEvent) {
		if (event.target === event.currentTarget) {
			onCancel();
		}
	}

	function handleConfirm() {
		onConfirm(requiresForce ? forceCheckbox : false);
	}

	let confidencePercent = $derived(
		learnedSchema ? Math.round(learnedSchema.confidence * 100) : 0
	);

	type BadgeVariant = 'blue' | 'purple' | 'green' | 'red' | 'yellow' | 'orange' | 'indigo' | 'gray';
	let confidenceBadgeVariant = $derived<BadgeVariant>(
		confidencePercent >= 95 ? 'green' : confidencePercent >= 85 ? 'blue' : 'yellow'
	);

	let canConfirm = $derived(
		!requiresForce || (requiresForce && forceCheckbox)
	);
</script>

{#if show && tool && learnedSchema}
	<div
		class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4"
		role="dialog"
		aria-modal="true"
		onclick={handleBackdropClick}
	>
		<div
			class="bg-white rounded-lg shadow-xl max-w-2xl w-full max-h-[90vh] overflow-y-auto"
			onclick={(e) => e.stopPropagation()}
		>
			<!-- Header -->
			<div class="bg-white border-b border-gray-200 px-6 py-4 flex items-center justify-between">
				<div class="flex items-center gap-3">
					<Sparkles class="w-6 h-6 text-blue-600" />
					<h2 class="text-xl font-semibold text-gray-900">Apply Learned Schema</h2>
				</div>
				<button
					type="button"
					onclick={onCancel}
					class="text-gray-500 hover:text-gray-700 transition-colors"
					aria-label="Close modal"
				>
					<X class="w-6 h-6" />
				</button>
			</div>

			<!-- Content -->
			<div class="px-6 py-6 space-y-6">
				<!-- Tool Info -->
				<div>
					<h3 class="text-sm font-medium text-gray-500 mb-2">Tool</h3>
					<div class="flex items-center gap-2">
						<span class="text-gray-900 font-medium">{tool.name}</span>
						{#if tool.httpMethod && tool.httpPath}
							<Badge variant="gray" size="sm">
								{tool.httpMethod} {tool.httpPath}
							</Badge>
						{/if}
					</div>
				</div>

				<!-- Schema Details -->
				<div class="bg-blue-50 border border-blue-200 rounded-lg p-4">
					<h3 class="text-sm font-medium text-blue-900 mb-3">Learned Schema Details</h3>
					<div class="grid grid-cols-2 gap-4">
						<div>
							<span class="text-xs text-blue-700">Confidence</span>
							<div class="mt-1">
								<Badge variant={confidenceBadgeVariant}>
									<CheckCircle2 class="w-3 h-3 mr-1" />
									{confidencePercent}%
								</Badge>
							</div>
						</div>
						<div>
							<span class="text-xs text-blue-700">Sample Count</span>
							<div class="mt-1 text-sm font-medium text-blue-900">
								{learnedSchema.sampleCount} samples
							</div>
						</div>
						<div>
							<span class="text-xs text-blue-700">Version</span>
							<div class="mt-1 text-sm font-medium text-blue-900">
								v{learnedSchema.version}
							</div>
						</div>
						<div>
							<span class="text-xs text-blue-700">Last Observed</span>
							<div class="mt-1 text-sm font-medium text-blue-900">
								{new Date(learnedSchema.lastObserved).toLocaleDateString()}
							</div>
						</div>
					</div>
				</div>

				<!-- What Will Change -->
				<div>
					<h3 class="text-sm font-medium text-gray-900 mb-3">What will change</h3>
					<div class="bg-gray-50 border border-gray-200 rounded-lg p-4 space-y-3">
						<div class="flex items-center gap-3">
							<div class="flex items-center gap-2 flex-1">
								<span class="text-sm text-gray-600">Current Source:</span>
								<Badge variant="gray" size="sm">
									{currentSource}
								</Badge>
							</div>
							<ArrowRight class="w-4 h-4 text-gray-400" />
							<div class="flex items-center gap-2 flex-1">
								<span class="text-sm text-gray-600">New Source:</span>
								<Badge variant="purple" size="sm">
									learned
								</Badge>
							</div>
						</div>
						<div class="text-sm text-gray-600">
							The tool's input and output schemas will be updated to match the learned schema based on {learnedSchema.sampleCount} observed samples.
						</div>
					</div>
				</div>

				<!-- OpenAPI Override Warning -->
				{#if requiresForce}
					<div class="bg-amber-50 border border-amber-200 rounded-lg p-4">
						<div class="flex items-start gap-3">
							<AlertTriangle class="w-5 h-5 text-amber-600 flex-shrink-0 mt-0.5" />
							<div class="flex-1">
								<h3 class="text-sm font-medium text-amber-900 mb-2">
									Override OpenAPI Schema
								</h3>
								<p class="text-sm text-amber-700 mb-4">
									This route currently uses an OpenAPI schema. Applying the learned schema will replace it. This action cannot be easily undone.
								</p>
								<label class="flex items-start gap-3 cursor-pointer">
									<input
										type="checkbox"
										bind:checked={forceCheckbox}
										class="mt-0.5 h-4 w-4 text-amber-600 border-amber-300 rounded focus:ring-amber-500"
									/>
									<span class="text-sm text-amber-900">
										I understand that this will replace the current OpenAPI schema
									</span>
								</label>
							</div>
						</div>
					</div>
				{/if}
			</div>

			<!-- Footer -->
			<div class="bg-gray-50 border-t border-gray-200 px-6 py-4 flex justify-end gap-3">
				<Button variant="ghost" onclick={onCancel}>
					Cancel
				</Button>
				<Button
					variant="primary"
					onclick={handleConfirm}
					disabled={!canConfirm}
				>
					<CheckCircle2 class="w-4 h-4 mr-2" />
					Apply Learned Schema
				</Button>
			</div>
		</div>
	</div>
{/if}
