<script lang="ts">
	import type { SecretResponse, SecretType } from '$lib/api/types';
	import { apiClient } from '$lib/api/client';
	import { selectedTeam } from '$lib/stores/team';
	import { onMount } from 'svelte';

	interface Props {
		/** The secret name value */
		value: string;
		/** Callback when the secret selection changes */
		onChange: (value: string) => void;
		/** Optional filter by secret type (from x-secret-type) */
		secretType?: SecretType;
		/** Optional label for the field */
		label?: string;
		/** Optional description for the field */
		description?: string;
		/** Whether the field is required */
		required?: boolean;
		/** Optional error messages */
		errors?: string[];
	}

	let { value, onChange, secretType, label, description, required = false, errors = [] }: Props = $props();

	// State for available secrets
	let secrets = $state<SecretResponse[]>([]);
	let loading = $state(true);
	let loadError = $state<string | null>(null);
	let currentTeam = $state<string>('');

	// Subscribe to team changes
	$effect(() => {
		const unsubscribe = selectedTeam.subscribe((team) => {
			if (team && team !== currentTeam) {
				currentTeam = team;
				loadSecrets(team);
			}
		});
		return unsubscribe;
	});

	async function loadSecrets(team: string) {
		if (!team) return;

		try {
			loading = true;
			loadError = null;
			secrets = await apiClient.listSecrets(team, {
				secret_type: secretType
			});
		} catch (e) {
			loadError = e instanceof Error ? e.message : 'Failed to load secrets';
			secrets = [];
		} finally {
			loading = false;
		}
	}

	onMount(() => {
		// Initial load if team is already set
		if (currentTeam) {
			loadSecrets(currentTeam);
		}
	});

	function handleSelectChange(e: Event) {
		const target = e.target as HTMLSelectElement;
		onChange(target.value);
	}

	function formatSecretType(type: SecretType): string {
		switch (type) {
			case 'generic_secret':
				return 'Generic';
			case 'tls_certificate':
				return 'TLS Cert';
			case 'certificate_validation_context':
				return 'CA Cert';
			case 'session_ticket_keys':
				return 'Session Keys';
			default:
				return type;
		}
	}

	const hasError = $derived(errors.length > 0);
	const selectClasses = $derived(
		`w-full px-3 py-2 text-sm border rounded-md focus:outline-none focus:ring-2 ${
			hasError
				? 'border-red-300 focus:ring-red-500 focus:border-red-500'
				: 'border-gray-300 focus:ring-blue-500 focus:border-blue-500'
		}`
	);
</script>

<div class="space-y-2">
	{#if label}
		<label class="flex items-center gap-1 text-sm font-medium text-gray-700">
			{label}
			{#if required}
				<span class="text-red-500">*</span>
			{/if}
		</label>
	{/if}

	{#if description}
		<p class="text-xs text-gray-500">{description}</p>
	{/if}

	{#if loading}
		<div class="flex items-center gap-2 text-sm text-gray-500">
			<svg class="animate-spin h-4 w-4" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
				<circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
				<path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
			</svg>
			Loading secrets...
		</div>
	{:else if loadError}
		<div class="text-sm text-red-600">
			{loadError}
		</div>
	{:else}
		<select
			value={value}
			onchange={handleSelectChange}
			class={selectClasses}
		>
			<option value="">Select a secret...</option>
			{#each secrets as secret}
				<option value={secret.name}>
					{secret.name} ({formatSecretType(secret.secret_type)})
				</option>
			{/each}
		</select>

		{#if secrets.length === 0}
			<p class="text-xs text-gray-500">
				No secrets found. <a href="/secrets/create" class="text-blue-600 hover:underline">Create one</a>
			</p>
		{/if}
	{/if}

	{#if errors.length > 0}
		<div class="space-y-1">
			{#each errors as error}
				<p class="text-xs text-red-600">{error}</p>
			{/each}
		</div>
	{/if}
</div>
