<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { ArrowLeft, Save, Server } from 'lucide-svelte';
	import { selectedTeam } from '$lib/stores/team';
	import Button from '$lib/components/Button.svelte';
	import {
		validateRequired,
		validateIdentifier,
		runValidators
	} from '$lib/utils/validators';

	let currentTeam = $state<string>('');

	// Subscribe to team changes
	selectedTeam.subscribe((value) => {
		currentTeam = value;
	});

	// Form state
	let formState = $state({
		name: '',
		gatewayHost: '',
		description: ''
	});

	let isSubmitting = $state(false);
	let error = $state<string | null>(null);

	// Validation
	function validateForm(): string | null {
		return runValidators([
			() => validateRequired(formState.name, 'Dataplane name'),
			() => validateIdentifier(formState.name, 'Dataplane name')
		]);
	}

	// Handle form submission
	async function handleSubmit(e: Event) {
		e.preventDefault();

		const validationError = validateForm();
		if (validationError) {
			error = validationError;
			return;
		}

		isSubmitting = true;
		error = null;

		try {
			await apiClient.createDataplane(currentTeam, {
				team: currentTeam,
				name: formState.name,
				gatewayHost: formState.gatewayHost || undefined,
				description: formState.description || undefined
			});

			goto('/dataplanes');
		} catch (err) {
			error = err instanceof Error ? err.message : 'Failed to create dataplane';
		} finally {
			isSubmitting = false;
		}
	}

	// Navigate back
	function handleBack() {
		goto('/dataplanes');
	}
</script>

<div class="w-full px-4 sm:px-6 lg:px-8 py-8 max-w-3xl mx-auto">
	<!-- Header -->
	<div class="mb-8">
		<button
			onclick={handleBack}
			class="flex items-center text-sm text-gray-600 hover:text-gray-900 mb-4"
		>
			<ArrowLeft class="h-4 w-4 mr-1" />
			Back to Dataplanes
		</button>

		<div class="flex items-center gap-3">
			<div class="p-3 bg-blue-100 rounded-lg">
				<Server class="h-6 w-6 text-blue-600" />
			</div>
			<div>
				<h1 class="text-2xl font-bold text-gray-900">Create Dataplane</h1>
				<p class="text-sm text-gray-600">
					Create a new Envoy instance for the <span class="font-medium">{currentTeam}</span> team
				</p>
			</div>
		</div>
	</div>

	<!-- Error Alert -->
	{#if error}
		<div class="bg-red-50 border border-red-200 rounded-md p-4 mb-6">
			<p class="text-sm text-red-800">{error}</p>
		</div>
	{/if}

	<!-- Form -->
	<form onsubmit={handleSubmit} class="space-y-6">
		<!-- Basic Information -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-medium text-gray-900 mb-4">Basic Information</h2>

			<div class="space-y-4">
				<!-- Name -->
				<div>
					<label for="name" class="block text-sm font-medium text-gray-700 mb-1">
						Name <span class="text-red-500">*</span>
					</label>
					<input
						type="text"
						id="name"
						bind:value={formState.name}
						placeholder="e.g., prod-gateway, staging-envoy"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						required
					/>
					<p class="mt-1 text-xs text-gray-500">
						A unique identifier for this dataplane. Use lowercase letters, numbers, and hyphens.
					</p>
				</div>

				<!-- Description -->
				<div>
					<label for="description" class="block text-sm font-medium text-gray-700 mb-1">
						Description
					</label>
					<textarea
						id="description"
						bind:value={formState.description}
						placeholder="Optional description for this dataplane"
						rows="2"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					></textarea>
				</div>
			</div>
		</div>

		<!-- Network Configuration -->
		<div class="bg-white rounded-lg shadow-sm border border-gray-200 p-6">
			<h2 class="text-lg font-medium text-gray-900 mb-4">Network Configuration</h2>

			<div class="space-y-4">
				<!-- Gateway Host -->
				<div>
					<label for="gatewayHost" class="block text-sm font-medium text-gray-700 mb-1">
						Gateway Host
					</label>
					<input
						type="text"
						id="gatewayHost"
						bind:value={formState.gatewayHost}
						placeholder="e.g., 10.0.0.5, host.docker.internal, envoy.service.consul"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<p class="mt-1 text-xs text-gray-500">
						The address where this Envoy instance is reachable from the Control Plane. Leave empty
						to use localhost (127.0.0.1). For Docker, use <code class="bg-gray-100 px-1 rounded"
							>host.docker.internal</code
						>. For Kubernetes, use the service DNS name.
					</p>
				</div>
			</div>
		</div>

		<!-- Form Actions -->
		<div class="flex justify-end gap-3">
			<Button onclick={handleBack} variant="secondary" disabled={isSubmitting}>
				Cancel
			</Button>
			<Button type="submit" variant="primary" disabled={isSubmitting}>
				{#if isSubmitting}
					<div class="animate-spin rounded-full h-4 w-4 border-b-2 border-white mr-2"></div>
					Creating...
				{:else}
					<Save class="h-4 w-4 mr-2" />
					Create Dataplane
				{/if}
			</Button>
		</div>
	</form>
</div>
