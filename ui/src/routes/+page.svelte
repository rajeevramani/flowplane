<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { apiClient } from '$lib/api/client';

	onMount(async () => {
		try {
			// Try to get session info
			await apiClient.getSessionInfo();
			// If successful, user is logged in - redirect to dashboard
			goto('/dashboard');
		} catch {
			// Not logged in - redirect to login
			goto('/login');
		}
	});
</script>

<div class="min-h-screen bg-gray-50 flex items-center justify-center p-4">
	<div class="text-center">
		<div class="animate-spin h-8 w-8 border-4 border-blue-600 border-t-transparent rounded-full mx-auto mb-4"></div>
		<p class="text-gray-600">Loading...</p>
	</div>
</div>
