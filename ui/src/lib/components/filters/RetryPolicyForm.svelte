<script lang="ts" module>
    // Retry presets
    export type RetryPreset =
        | "5xx"
        | "connection"
        | "gateway"
        | "all"
        | "custom";

    export const retryPresets: {
        value: RetryPreset;
        label: string;
        conditions: string[];
    }[] = [
        { value: "5xx", label: "5xx Server Errors", conditions: ["5xx"] },
        {
            value: "connection",
            label: "Connection Failures",
            conditions: ["reset", "connect-failure"],
        },
        {
            value: "gateway",
            label: "Gateway Errors",
            conditions: ["gateway-error"],
        },
        {
            value: "all",
            label: "All Retriable",
            conditions: [
                "5xx",
                "reset",
                "connect-failure",
                "retriable-4xx",
                "refused-stream",
                "gateway-error",
            ],
        },
        { value: "custom", label: "Custom...", conditions: [] },
    ];

    export const retryConditions = [
        { value: "5xx", label: "5xx Server Errors" },
        { value: "reset", label: "Connection Reset" },
        { value: "connect-failure", label: "Connect Failure" },
        { value: "retriable-4xx", label: "Retriable 4xx" },
        { value: "refused-stream", label: "Refused Stream" },
        { value: "gateway-error", label: "Gateway Error" },
    ];

    // Helper: detect preset from conditions array
    export function detectPresetFromConditions(
        conditions: string[],
    ): RetryPreset {
        const sorted = [...conditions].sort();
        for (const preset of retryPresets) {
            if (preset.value === "custom") continue;
            const presetSorted = [...preset.conditions].sort();
            if (
                sorted.length === presetSorted.length &&
                sorted.every((c, i) => c === presetSorted[i])
            ) {
                return preset.value;
            }
        }
        return "custom";
    }
</script>

<script lang="ts">
    interface Props {
        retryEnabled: boolean;
        maxRetries: number;
        preset: RetryPreset;
        customConditions: string[];
        perTryTimeout: number | null;
        showBackoff: boolean;
        backoffBaseInterval: number;
        backoffMaxInterval: number;
    }

    let {
        retryEnabled = $bindable(),
        maxRetries = $bindable(),
        preset = $bindable(),
        customConditions = $bindable(),
        perTryTimeout = $bindable(),
        showBackoff = $bindable(),
        backoffBaseInterval = $bindable(),
        backoffMaxInterval = $bindable(),
    }: Props = $props();

    // Helper: toggle retry condition in custom mode
    function toggleRetryCondition(condition: string) {
        if (customConditions.includes(condition)) {
            customConditions = customConditions.filter((c) => c !== condition);
        } else {
            customConditions = [...customConditions, condition];
        }
    }
</script>

<div class="space-y-4">
    <!-- Retry Policy Toggle -->
    <div class="flex items-center gap-3">
        <input
            type="checkbox"
            id="retry-enabled"
            bind:checked={retryEnabled}
            class="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
        />
        <label for="retry-enabled" class="text-sm font-medium text-gray-700">
            Enable Retry Policy
        </label>
    </div>

    {#if retryEnabled}
        <div class="pl-7 space-y-4">
            <!-- Max Retries -->
            <div class="flex items-center gap-4">
                <div>
                    <label
                        for="max-retries"
                        class="block text-sm font-medium text-gray-700 mb-1"
                    >
                        Max Retries
                    </label>
                    <select
                        id="max-retries"
                        bind:value={maxRetries}
                        class="w-20 rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
                    >
                        {#each [1, 2, 3, 4, 5, 6, 7, 8, 9, 10] as n}
                            <option value={n}>{n}</option>
                        {/each}
                    </select>
                </div>
            </div>

            <!-- Retry Preset -->
            <div>
                <label
                    for="retry-preset"
                    class="block text-sm font-medium text-gray-700 mb-1"
                >
                    Retry On
                </label>
                <select
                    id="retry-preset"
                    bind:value={preset}
                    class="w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
                >
                    {#each retryPresets as p}
                        <option value={p.value}>{p.label}</option>
                    {/each}
                </select>
            </div>

            <!-- Custom Conditions -->
            {#if preset === "custom"}
                <div class="bg-gray-50 p-3 rounded-md">
                    <label class="block text-sm font-medium text-gray-700 mb-2">
                        Select Retry Conditions
                    </label>
                    <div class="grid grid-cols-2 gap-2">
                        {#each retryConditions as condition}
                            <label class="flex items-center gap-2 text-sm">
                                <input
                                    type="checkbox"
                                    checked={customConditions.includes(
                                        condition.value,
                                    )}
                                    onchange={() =>
                                        toggleRetryCondition(condition.value)}
                                    class="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                                />
                                {condition.label}
                            </label>
                        {/each}
                    </div>
                    {#if customConditions.length === 0}
                        <p class="mt-2 text-xs text-amber-600">
                            Select at least one retry condition
                        </p>
                    {/if}
                </div>
            {/if}

            <!-- Per-Try Timeout -->
            <div>
                <label
                    for="per-try-timeout"
                    class="block text-sm font-medium text-gray-700 mb-1"
                >
                    Per-Try Timeout
                    <span class="text-xs text-gray-400"
                        >(optional, seconds)</span
                    >
                </label>
                <input
                    id="per-try-timeout"
                    type="number"
                    min="1"
                    max="300"
                    bind:value={perTryTimeout}
                    placeholder="Use route timeout"
                    class="w-32 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
                />
            </div>

            <!-- Backoff Settings (Collapsible) -->
            <div class="border-t border-gray-200 pt-4">
                <button
                    type="button"
                    onclick={() => (showBackoff = !showBackoff)}
                    class="flex items-center gap-2 text-sm font-medium text-gray-700 hover:text-gray-900"
                >
                    <svg
                        class="h-4 w-4 transition-transform {showBackoff
                            ? 'rotate-90'
                            : ''}"
                        fill="none"
                        stroke="currentColor"
                        viewBox="0 0 24 24"
                    >
                        <path
                            stroke-linecap="round"
                            stroke-linejoin="round"
                            stroke-width="2"
                            d="M9 5l7 7-7 7"
                        />
                    </svg>
                    Backoff Settings
                    <span class="text-xs text-gray-400">(optional)</span>
                </button>

                {#if showBackoff}
                    <div class="mt-3 pl-6 space-y-3">
                        <div>
                            <label
                                for="backoff-base"
                                class="block text-sm text-gray-600 mb-1"
                            >
                                Base Interval (ms)
                            </label>
                            <input
                                id="backoff-base"
                                type="number"
                                min="10"
                                max="10000"
                                bind:value={backoffBaseInterval}
                                class="w-28 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
                            />
                        </div>
                        <div>
                            <label
                                for="backoff-max"
                                class="block text-sm text-gray-600 mb-1"
                            >
                                Max Interval (ms)
                            </label>
                            <input
                                id="backoff-max"
                                type="number"
                                min="100"
                                max="60000"
                                bind:value={backoffMaxInterval}
                                class="w-28 rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
                            />
                        </div>
                        {#if backoffBaseInterval >= backoffMaxInterval}
                            <p class="text-xs text-amber-600">
                                Base interval should be less than max interval
                            </p>
                        {/if}
                    </div>
                {/if}
            </div>
        </div>
    {:else}
        <p class="pl-7 text-sm text-gray-500">
            Enable retry policy to configure automatic request retries on
            failures.
        </p>
    {/if}
</div>
