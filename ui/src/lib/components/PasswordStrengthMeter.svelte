<script lang="ts">
	interface Props {
		password: string;
	}

	let { password }: Props = $props();

	const strength = $derived(() => {
		if (!password) return { score: 0, label: '', color: '' };

		let score = 0;
		if (password.length >= 8) score++;
		if (password.length >= 12) score++;
		if (/[a-z]/.test(password)) score++;
		if (/[A-Z]/.test(password)) score++;
		if (/[0-9]/.test(password)) score++;
		if (/[^a-zA-Z0-9]/.test(password)) score++;

		const capped = Math.min(score, 4);
		const labels = ['Weak', 'Fair', 'Good', 'Strong'];
		const colors = ['bg-red-500', 'bg-orange-500', 'bg-yellow-500', 'bg-green-500'];

		return {
			score: capped,
			label: labels[capped - 1] || '',
			color: colors[capped - 1] || ''
		};
	});
</script>

{#if password.length > 0}
	<div class="mt-2">
		<div class="flex items-center gap-2">
			<div class="flex-1 h-2 bg-gray-200 rounded-full overflow-hidden">
				<div
					class="h-full transition-all duration-300 {strength().color}"
					style="width: {(strength().score / 4) * 100}%"
				></div>
			</div>
			<span class="text-xs text-gray-600">{strength().label}</span>
		</div>
		<p class="mt-1 text-xs text-gray-500">
			Use at least 8 characters with a mix of letters, numbers, and symbols
		</p>
	</div>
{/if}
