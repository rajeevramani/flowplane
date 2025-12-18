<script lang="ts">
	import type { ListenerResponse, ListenerTlsContextInput } from '$lib/api/types';
	import { ChevronDown, ChevronRight } from 'lucide-svelte';
	import TlsConfigForm from './listener/TlsConfigForm.svelte';

	export interface ListenerConfig {
		mode: 'existing' | 'new';
		existingListenerName?: string;
		newAddress: string;
		newPort: number;
		tlsContext?: ListenerTlsContextInput | null;
	}

	interface Props {
		listeners: ListenerResponse[];
		config: ListenerConfig;
		onConfigChange: (config: ListenerConfig) => void;
	}

	let { listeners, config, onConfigChange }: Props = $props();

	// State for advanced configuration section
	let showAdvancedConfig = $state(false);

	// Derived: check if TLS is configured
	let hasTlsConfig = $derived(
		config.mode === 'new' && config.tlsContext !== undefined && config.tlsContext !== null
	);

	function handleModeChange(mode: 'existing' | 'new') {
		onConfigChange({ ...config, mode });
	}

	function handleExistingListenerChange(e: Event) {
		const target = e.target as HTMLSelectElement;
		onConfigChange({ ...config, existingListenerName: target.value });
	}

	function handleAddressChange(e: Event) {
		const target = e.target as HTMLInputElement;
		onConfigChange({ ...config, newAddress: target.value });
	}

	function handlePortChange(e: Event) {
		const target = e.target as HTMLInputElement;
		onConfigChange({ ...config, newPort: Number(target.value) });
	}

	function handleTlsContextChange(context: ListenerTlsContextInput | null) {
		onConfigChange({ ...config, tlsContext: context });
	}

	function toggleAdvancedConfig() {
		showAdvancedConfig = !showAdvancedConfig;
	}
</script>

<div class="space-y-4">
	<label class="block text-sm font-medium text-gray-700">Listener Configuration</label>

	<div class="space-y-3">
		<label class="flex items-center gap-3 cursor-pointer">
			<input
				type="radio"
				name="listener-mode"
				checked={config.mode === 'existing'}
				onchange={() => handleModeChange('existing')}
				class="h-4 w-4 text-blue-600 focus:ring-blue-500"
			/>
			<span class="text-sm text-gray-700">Use existing listener</span>
		</label>

		{#if config.mode === 'existing'}
			<div class="ml-7">
				{#if listeners.length === 0}
					<p class="text-sm text-gray-500 italic">No existing listeners available</p>
				{:else}
					<select
						value={config.existingListenerName ?? ''}
						onchange={handleExistingListenerChange}
						class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
					>
						<option value="">Select a listener...</option>
						{#each listeners as listener}
							<option value={listener.name}>
								{listener.name} ({listener.address}:{listener.port})
							</option>
						{/each}
					</select>
				{/if}
			</div>
		{/if}

		<label class="flex items-center gap-3 cursor-pointer">
			<input
				type="radio"
				name="listener-mode"
				checked={config.mode === 'new'}
				onchange={() => handleModeChange('new')}
				class="h-4 w-4 text-blue-600 focus:ring-blue-500"
			/>
			<span class="text-sm text-gray-700">Create new listener</span>
		</label>

		{#if config.mode === 'new'}
			<div class="ml-7 space-y-4">
				<div class="flex items-center gap-3">
					<div class="flex-1">
						<label for="listener-address" class="block text-xs text-gray-500 mb-1">Address</label>
						<input
							id="listener-address"
							type="text"
							placeholder="0.0.0.0"
							value={config.newAddress}
							oninput={handleAddressChange}
							class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
						/>
					</div>
					<div class="w-32">
						<label for="listener-port" class="block text-xs text-gray-500 mb-1">Port</label>
						<input
							id="listener-port"
							type="number"
							min="1024"
							max="65535"
							placeholder="8080"
							value={config.newPort}
							oninput={handlePortChange}
							class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
						/>
					</div>
				</div>

				<!-- Advanced Configuration (Collapsible) -->
				<div class="border-t border-gray-200 pt-4">
					<button
						type="button"
						onclick={toggleAdvancedConfig}
						class="flex items-center gap-2 text-sm font-medium text-gray-700 hover:text-gray-900 transition-colors"
					>
						{#if showAdvancedConfig}
							<ChevronDown class="h-4 w-4" />
						{:else}
							<ChevronRight class="h-4 w-4" />
						{/if}
						Advanced Configuration
						{#if hasTlsConfig}
							<span class="ml-2 px-2 py-0.5 text-xs bg-green-100 text-green-700 rounded">TLS Configured</span>
						{/if}
					</button>

					{#if showAdvancedConfig}
						<div class="mt-4 pl-6 border-l-2 border-blue-200">
							<h4 class="text-sm font-medium text-gray-700 mb-3">TLS Configuration</h4>
							<TlsConfigForm
								tlsContext={config.tlsContext || null}
								onTlsContextChange={handleTlsContextChange}
								compact={true}
							/>
						</div>
					{/if}
				</div>
			</div>
		{/if}
	</div>
</div>
