<script lang="ts">
	import type { ListenerFilterChainInput } from '$lib/api/types';
	import FilterChainList from './listener/FilterChainList.svelte';
	import { AlertTriangle } from 'lucide-svelte';

	interface Props {
		// Basic listener info
		address: string;
		port: number;
		protocol?: string;

		// Filter chains with TLS
		filterChains: ListenerFilterChainInput[];

		// Callbacks
		onAddressChange: (address: string) => void;
		onPortChange: (port: number) => void;
		onProtocolChange: (protocol: string) => void;
		onFilterChainsChange: (chains: ListenerFilterChainInput[]) => void;

		// Options
		compact?: boolean;
		showAddressPortWarning?: boolean; // Show warning in edit mode about listener restart
	}

	let {
		address,
		port,
		protocol = 'HTTP',
		filterChains,
		onAddressChange,
		onPortChange,
		onProtocolChange,
		onFilterChainsChange,
		compact = false,
		showAddressPortWarning = false
	}: Props = $props();

	let warningDismissed = $state(false);

	function handleAddressInput(e: Event) {
		const target = e.target as HTMLInputElement;
		onAddressChange(target.value);
	}

	function handlePortInput(e: Event) {
		const target = e.target as HTMLInputElement;
		onPortChange(Number(target.value));
	}

	function handleProtocolChange(e: Event) {
		const target = e.target as HTMLSelectElement;
		onProtocolChange(target.value);
	}
</script>

<div class="space-y-6">
	<!-- Address/Port Section -->
	<div class="space-y-4">
		<h3 class="text-sm font-medium text-gray-700">Listener Binding</h3>

		<!-- Warning for edit mode -->
		{#if showAddressPortWarning && !warningDismissed}
			<div class="flex items-start gap-3 p-3 bg-yellow-50 border border-yellow-200 rounded-md">
				<AlertTriangle class="h-5 w-5 text-yellow-600 flex-shrink-0 mt-0.5" />
				<div class="flex-1">
					<p class="text-sm text-yellow-800">
						<strong>Warning:</strong> Changing the address or port will restart the listener.
						Active connections will be gracefully drained, but clients will need to reconnect to the new address/port.
					</p>
				</div>
				<button
					type="button"
					onclick={() => warningDismissed = true}
					class="text-yellow-600 hover:text-yellow-800 text-sm font-medium"
				>
					Dismiss
				</button>
			</div>
		{/if}

		<div class="flex items-start gap-4">
			<div class="flex-1">
				<label for="listener-address" class="block text-sm font-medium text-gray-700 mb-1">
					Address
				</label>
				<input
					id="listener-address"
					type="text"
					placeholder="0.0.0.0"
					value={address}
					oninput={handleAddressInput}
					class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm font-mono focus:ring-1 focus:ring-blue-500"
				/>
			</div>

			<div class="w-32">
				<label for="listener-port" class="block text-sm font-medium text-gray-700 mb-1">
					Port
				</label>
				<input
					id="listener-port"
					type="number"
					min="1024"
					max="65535"
					placeholder="8080"
					value={port}
					oninput={handlePortInput}
					class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:ring-1 focus:ring-blue-500"
				/>
			</div>
		</div>

		<div class="w-48">
			<label for="listener-protocol" class="block text-sm font-medium text-gray-700 mb-1">
				Protocol
			</label>
			<select
				id="listener-protocol"
				value={protocol}
				onchange={handleProtocolChange}
				class="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:ring-1 focus:ring-blue-500"
			>
				<option value="HTTP">HTTP</option>
				<option value="TCP">TCP</option>
			</select>
		</div>
	</div>

	<!-- Filter Chains Section -->
	<div class="space-y-4">
		<div class="flex items-center justify-between">
			<h3 class="text-sm font-medium text-gray-700">Filter Chains</h3>
			<span class="text-xs text-gray-500">{filterChains.length} chain{filterChains.length !== 1 ? 's' : ''}</span>
		</div>

		<FilterChainList
			{filterChains}
			onFilterChainsChange={onFilterChainsChange}
			{compact}
		/>
	</div>
</div>
