<script lang="ts">
	interface Props {
		variant?: "primary" | "secondary" | "danger" | "ghost";
		size?: "sm" | "md" | "lg";
		type?: "button" | "submit" | "reset";
		disabled?: boolean;
		onclick?: (event: MouseEvent) => void;
		children: any;
	}

	let {
		variant = "primary",
		size = "md",
		type = "button",
		disabled = false,
		onclick,
		children,
	}: Props = $props();

	const variantClasses = {
		primary: "text-white bg-blue-600 hover:bg-blue-700 focus:ring-blue-500",
		secondary:
			"text-blue-600 bg-white border border-blue-300 hover:bg-blue-50 focus:ring-blue-500",
		danger: "text-white bg-red-600 hover:bg-red-700 focus:ring-red-500",
		ghost: "text-gray-700 bg-gray-100 hover:bg-gray-200 focus:ring-gray-500",
	};

	const sizeClasses = {
		sm: "px-3 py-1.5 text-sm",
		md: "px-4 py-2 text-sm",
		lg: "px-6 py-3 text-base",
	};

	const baseClasses =
		"font-medium rounded-md focus:outline-none focus:ring-2 focus:ring-offset-2 transition-colors";
	let disabledClasses = $derived(
		disabled ? "opacity-50 cursor-not-allowed" : "",
	);
</script>

<button
	{type}
	{disabled}
	{onclick}
	class="{baseClasses} {variantClasses[variant]} {sizeClasses[
		size
	]} {disabledClasses}"
>
	{@render children()}
</button>
