<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { CreateOrganizationRequest } from '$lib/api/types';
	import { ErrorAlert, FormActions, PageHeader } from '$lib/components/forms';
	import { validateRequired, validateMaxLength, runValidators } from '$lib/utils/validators';
	import { isSystemAdmin } from '$lib/stores/org';

	let formData = $state({
		name: '',
		displayName: '',
		description: ''
	});

	let errors = $state<Record<string, string>>({});
	let isSubmitting = $state(false);
	let error = $state<string | null>(null);

	onMount(async () => {
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!isSystemAdmin(sessionInfo.scopes)) {
				goto('/dashboard');
				return;
			}
		} catch {
			goto('/login');
		}
	});

	function validateForm(): boolean {
		const newErrors: Record<string, string> = {};

		const nameError = runValidators([
			() => validateRequired(formData.name, 'Organization name'),
			() => validateMaxLength(formData.name, 255, 'Organization name')
		]);
		if (nameError) {
			newErrors.name = nameError;
		} else if (!/^[a-z0-9-]+$/.test(formData.name)) {
			newErrors.name = 'Organization name must be lowercase alphanumeric with hyphens only';
		}

		const displayNameError = runValidators([
			() => validateRequired(formData.displayName, 'Display name'),
			() => validateMaxLength(formData.displayName, 255, 'Display name')
		]);
		if (displayNameError) {
			newErrors.displayName = displayNameError;
		}

		if (formData.description && formData.description.length > 1000) {
			newErrors.description = 'Description must be 1000 characters or less';
		}

		errors = newErrors;
		return Object.keys(newErrors).length === 0;
	}

	async function handleSubmit() {
		if (!validateForm()) {
			return;
		}

		isSubmitting = true;
		error = null;

		try {
			const request: CreateOrganizationRequest = {
				name: formData.name,
				displayName: formData.displayName,
				description: formData.description || undefined
			};

			const org = await apiClient.createOrganization(request);
			goto(`/admin/organizations/${org.id}`);
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to create organization';
		} finally {
			isSubmitting = false;
		}
	}

	function handleCancel() {
		goto('/admin/organizations');
	}
</script>

<div class="max-w-2xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
	<PageHeader title="Create Organization" onBack={handleCancel} />

	<ErrorAlert message={error} />

	<div class="bg-white rounded-lg shadow-md p-6">
		<form
			onsubmit={(e) => {
				e.preventDefault();
				handleSubmit();
			}}
		>
			<div class="space-y-6">
				<!-- Organization Name -->
				<div>
					<label for="name" class="block text-sm font-medium text-gray-700 mb-2">
						Organization Name <span class="text-red-500">*</span>
					</label>
					<input
						id="name"
						type="text"
						bind:value={formData.name}
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.name
							? 'border-red-500'
							: ''}"
						placeholder="acme-corp"
					/>
					{#if errors.name}
						<p class="mt-1 text-sm text-red-600">{errors.name}</p>
					{:else}
						<p class="mt-1 text-xs text-gray-500">
							Immutable identifier (lowercase, alphanumeric with hyphens)
						</p>
					{/if}
				</div>

				<!-- Display Name -->
				<div>
					<label for="displayName" class="block text-sm font-medium text-gray-700 mb-2">
						Display Name <span class="text-red-500">*</span>
					</label>
					<input
						id="displayName"
						type="text"
						bind:value={formData.displayName}
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.displayName
							? 'border-red-500'
							: ''}"
						placeholder="Acme Corporation"
					/>
					{#if errors.displayName}
						<p class="mt-1 text-sm text-red-600">{errors.displayName}</p>
					{:else}
						<p class="mt-1 text-xs text-gray-500">Human-friendly name (can be changed later)</p>
					{/if}
				</div>

				<!-- Description -->
				<div>
					<label for="description" class="block text-sm font-medium text-gray-700 mb-2">
						Description
					</label>
					<textarea
						id="description"
						bind:value={formData.description}
						rows="3"
						class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.description
							? 'border-red-500'
							: ''}"
						placeholder="Organization description..."
					></textarea>
					{#if errors.description}
						<p class="mt-1 text-sm text-red-600">{errors.description}</p>
					{/if}
				</div>
			</div>
		</form>
	</div>

	<FormActions
		{isSubmitting}
		submitLabel="Create Organization"
		submittingLabel="Creating..."
		onSubmit={handleSubmit}
		onCancel={handleCancel}
	/>
</div>
