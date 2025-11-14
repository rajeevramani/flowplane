<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { CreateUserRequest } from '$lib/api/types';

	let formData = $state({
		email: '',
		name: '',
		password: '',
		confirmPassword: '',
		isAdmin: false
	});

	let errors = $state<Record<string, string>>({});
	let isSubmitting = $state(false);
	let submitError = $state<string | null>(null);

	onMount(async () => {
		// Check authentication and admin access
		try {
			const sessionInfo = await apiClient.getSessionInfo();
			if (!sessionInfo.isAdmin) {
				goto('/dashboard');
				return;
			}
		} catch (err) {
			goto('/login');
		}
	});

	function getPasswordStrength(password: string): {
		score: number;
		label: string;
		color: string;
	} {
		if (password.length === 0) {
			return { score: 0, label: '', color: '' };
		}

		let score = 0;

		// Length
		if (password.length >= 8) score++;
		if (password.length >= 12) score++;

		// Contains lowercase
		if (/[a-z]/.test(password)) score++;

		// Contains uppercase
		if (/[A-Z]/.test(password)) score++;

		// Contains numbers
		if (/[0-9]/.test(password)) score++;

		// Contains special characters
		if (/[^a-zA-Z0-9]/.test(password)) score++;

		// Determine label and color
		if (score <= 2) {
			return { score, label: 'Weak', color: 'bg-red-500' };
		} else if (score <= 4) {
			return { score, label: 'Medium', color: 'bg-yellow-500' };
		} else {
			return { score, label: 'Strong', color: 'bg-green-500' };
		}
	}

	let passwordStrength = $derived.by(() => getPasswordStrength(formData.password));

	function validateForm(): boolean {
		const newErrors: Record<string, string> = {};

		// Email validation
		if (!formData.email.trim()) {
			newErrors.email = 'Email is required';
		} else if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(formData.email)) {
			newErrors.email = 'Invalid email format';
		}

		// Name validation
		if (!formData.name.trim()) {
			newErrors.name = 'Name is required';
		}

		// Password validation
		if (!formData.password) {
			newErrors.password = 'Password is required';
		} else if (formData.password.length < 8) {
			newErrors.password = 'Password must be at least 8 characters';
		}

		// Confirm password validation
		if (formData.password !== formData.confirmPassword) {
			newErrors.confirmPassword = 'Passwords do not match';
		}

		errors = newErrors;
		return Object.keys(newErrors).length === 0;
	}

	async function handleSubmit() {
		if (!validateForm()) {
			return;
		}

		isSubmitting = true;
		submitError = null;

		try {
			const request: CreateUserRequest = {
				email: formData.email,
				name: formData.name,
				password: formData.password,
				isAdmin: formData.isAdmin
			};

			const user = await apiClient.createUser(request);

			// Navigate to user detail page
			goto(`/admin/users/${user.id}`);
		} catch (err: any) {
			submitError = err.message || 'Failed to create user';
		} finally {
			isSubmitting = false;
		}
	}

	function handleCancel() {
		goto('/admin/users');
	}
</script>

<div class="min-h-screen bg-gray-50">
	<!-- Navigation -->
	<nav class="bg-white shadow-sm border-b border-gray-200">
		<div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
			<div class="flex justify-between h-16 items-center">
				<div class="flex items-center gap-4">
					<a
						href="/admin/users"
						class="text-blue-600 hover:text-blue-800"
						aria-label="Back to users"
					>
						<svg class="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M10 19l-7-7m0 0l7-7m-7 7h18"
							/>
						</svg>
					</a>
					<h1 class="text-xl font-bold text-gray-900">Create User</h1>
				</div>
			</div>
		</div>
	</nav>

	<main class="max-w-2xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
		<!-- Error Message -->
		{#if submitError}
			<div class="mb-6 bg-red-50 border-l-4 border-red-500 rounded-md p-4">
				<p class="text-red-800 text-sm">{submitError}</p>
			</div>
		{/if}

		<!-- Create User Form -->
		<div class="bg-white rounded-lg shadow-md p-6">
			<form onsubmit={(e) => { e.preventDefault(); handleSubmit(); }}>
				<div class="space-y-6">
					<!-- Email -->
					<div>
						<label for="email" class="block text-sm font-medium text-gray-700 mb-2">
							Email <span class="text-red-500">*</span>
						</label>
						<input
							id="email"
							type="email"
							bind:value={formData.email}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.email
								? 'border-red-500'
								: ''}"
							placeholder="user@example.com"
						/>
						{#if errors.email}
							<p class="mt-1 text-sm text-red-600">{errors.email}</p>
						{/if}
					</div>

					<!-- Name -->
					<div>
						<label for="name" class="block text-sm font-medium text-gray-700 mb-2">
							Full Name <span class="text-red-500">*</span>
						</label>
						<input
							id="name"
							type="text"
							bind:value={formData.name}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.name
								? 'border-red-500'
								: ''}"
							placeholder="John Doe"
						/>
						{#if errors.name}
							<p class="mt-1 text-sm text-red-600">{errors.name}</p>
						{/if}
					</div>

					<!-- Password -->
					<div>
						<label for="password" class="block text-sm font-medium text-gray-700 mb-2">
							Password <span class="text-red-500">*</span>
						</label>
						<input
							id="password"
							type="password"
							bind:value={formData.password}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.password
								? 'border-red-500'
								: ''}"
							placeholder="••••••••"
						/>
						{#if errors.password}
							<p class="mt-1 text-sm text-red-600">{errors.password}</p>
						{/if}

						<!-- Password Strength Meter -->
						{#if formData.password}
							<div class="mt-2">
								<div class="flex items-center gap-2 mb-1">
									<div class="flex-1 h-2 bg-gray-200 rounded-full overflow-hidden">
										<div
											class="h-full {passwordStrength.color} transition-all duration-300"
											style="width: {(passwordStrength.score / 6) * 100}%"
										></div>
									</div>
									{#if passwordStrength.label}
										<span class="text-xs text-gray-600">{passwordStrength.label}</span>
									{/if}
								</div>
								<p class="text-xs text-gray-500">
									Password should be at least 8 characters with uppercase, lowercase, numbers, and
									special characters
								</p>
							</div>
						{/if}
					</div>

					<!-- Confirm Password -->
					<div>
						<label for="confirmPassword" class="block text-sm font-medium text-gray-700 mb-2">
							Confirm Password <span class="text-red-500">*</span>
						</label>
						<input
							id="confirmPassword"
							type="password"
							bind:value={formData.confirmPassword}
							class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 {errors.confirmPassword
								? 'border-red-500'
								: ''}"
							placeholder="••••••••"
						/>
						{#if errors.confirmPassword}
							<p class="mt-1 text-sm text-red-600">{errors.confirmPassword}</p>
						{/if}
					</div>

					<!-- Role Selection -->
					<div>
						<label class="block text-sm font-medium text-gray-700 mb-3">Role</label>
						<div class="space-y-3">
							<label class="flex items-start p-3 border border-gray-200 rounded-md cursor-pointer hover:bg-gray-50">
								<input
									type="radio"
									bind:group={formData.isAdmin}
									value={false}
									class="mt-1 h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300"
								/>
								<div class="ml-3">
									<div class="text-sm font-medium text-gray-900">Developer</div>
									<div class="text-xs text-gray-500">
										Team-scoped access for API development and configuration
									</div>
								</div>
							</label>
							<label class="flex items-start p-3 border border-gray-200 rounded-md cursor-pointer hover:bg-gray-50">
								<input
									type="radio"
									bind:group={formData.isAdmin}
									value={true}
									class="mt-1 h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300"
								/>
								<div class="ml-3">
									<div class="text-sm font-medium text-gray-900">Administrator</div>
									<div class="text-xs text-gray-500">
										Full system access including user and team management
									</div>
								</div>
							</label>
						</div>
					</div>
				</div>

				<!-- Form Actions -->
				<div class="mt-6 flex justify-end gap-3">
					<button
						type="button"
						onclick={handleCancel}
						disabled={isSubmitting}
						class="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 disabled:opacity-50"
					>
						Cancel
					</button>
					<button
						type="submit"
						disabled={isSubmitting}
						class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50"
					>
						{isSubmitting ? 'Creating...' : 'Create User'}
					</button>
				</div>
			</form>
		</div>
	</main>
</div>
