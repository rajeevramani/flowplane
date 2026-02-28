<script lang="ts">
	const ZITADEL_ISSUER = import.meta.env.VITE_ZITADEL_ISSUER || 'http://localhost:8081';
	const accountUrl = `${ZITADEL_ISSUER}/ui/console/users/me`;
</script>

<div class="max-w-2xl mx-auto">
	<div class="mb-6">
		<h1 class="text-2xl font-bold text-gray-900">Change Password</h1>
		<p class="mt-1 text-sm text-gray-600">
			Password management is handled by your identity provider.
		</p>
	</div>

	<div class="bg-white rounded-lg shadow p-6">
		<p class="text-sm text-gray-700 mb-4">
			To change your password, visit your account settings in the identity provider.
		</p>
		<div class="flex gap-3">
			<a
				href={accountUrl}
				target="_blank"
				rel="noopener noreferrer"
				class="flex-1 flex justify-center py-2 px-4 border border-transparent rounded-md shadow-sm text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
			>
				Manage Account
			</a>
			<a
				href="/dashboard"
				class="flex-1 flex justify-center py-2 px-4 border border-gray-300 rounded-md shadow-sm text-sm font-medium text-gray-700 bg-white hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
			>
				Back to Dashboard
			</a>
		</div>
	</div>
</div>
