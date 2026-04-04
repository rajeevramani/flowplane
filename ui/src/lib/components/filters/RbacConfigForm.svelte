<script lang="ts">
	import type { RbacConfig, RbacRulesConfig, RbacPolicy, RbacAction } from '$lib/api/types';
	import { RbacConfigSchema } from '$lib/schemas/filter-configs';
	import { Info, Plus, Trash2, Settings, ChevronRight } from 'lucide-svelte';

	interface Props {
		config: RbacConfig;
		onConfigChange: (config: RbacConfig) => void;
	}

	let { config, onConfigChange }: Props = $props();

	// Rules mode
	let rulesAction = $state<RbacAction>(config.rules?.action ?? 'allow');
	let rulesStatPrefix = $state(config.rules_stat_prefix ?? '');
	let policies = $state<Record<string, RbacPolicy>>(
		config.rules?.policies ?? {}
	);

	// Shadow rules
	let enableShadowRules = $state(config.shadow_rules !== undefined);
	let shadowAction = $state<RbacAction>(config.shadow_rules?.action ?? 'log');
	let shadowStatPrefix = $state(config.shadow_rules_stat_prefix ?? '');
	let shadowPolicies = $state<Record<string, RbacPolicy>>(
		config.shadow_rules?.policies ?? {}
	);

	// Options
	let trackPerRuleStats = $state(config.track_per_rule_stats ?? false);

	// Advanced
	let showAdvanced = $state(false);

	// New policy
	let newPolicyName = $state('');
	let newShadowPolicyName = $state('');

	// Validation errors
	let validationErrors = $state<string[]>([]);

	const RBAC_ACTIONS: { value: RbacAction; label: string; description: string }[] = [
		{ value: 'allow', label: 'Allow', description: 'Allow requests matching the policy' },
		{ value: 'deny', label: 'Deny', description: 'Deny requests matching the policy' },
		{ value: 'log', label: 'Log', description: 'Log matching requests (shadow mode)' }
	];

	const PERMISSION_TYPES = [
		{ value: 'any', label: 'Any (match all)' },
		{ value: 'header', label: 'Header match' },
		{ value: 'url_path', label: 'URL path match' },
		{ value: 'destination_port', label: 'Destination port' }
	];

	const PRINCIPAL_TYPES = [
		{ value: 'any', label: 'Any (match all)' },
		{ value: 'authenticated', label: 'Authenticated' },
		{ value: 'source_ip', label: 'Source IP' },
		{ value: 'direct_remote_ip', label: 'Direct remote IP' },
		{ value: 'header', label: 'Header match' }
	];

	function updateParent() {
		const rules: RbacRulesConfig = {
			action: rulesAction,
			policies: { ...policies }
		};

		const cfg: RbacConfig = {
			rules,
			rules_stat_prefix: rulesStatPrefix || undefined,
			shadow_rules: enableShadowRules
				? {
						action: shadowAction,
						policies: { ...shadowPolicies }
					}
				: undefined,
			shadow_rules_stat_prefix: enableShadowRules && shadowStatPrefix ? shadowStatPrefix : undefined,
			track_per_rule_stats: trackPerRuleStats || undefined
		};

		const result = RbacConfigSchema.safeParse(cfg);
		validationErrors = result.success
			? []
			: result.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`);

		onConfigChange(cfg);
	}

	function addPolicy() {
		const name = newPolicyName.trim();
		if (name && !(name in policies)) {
			policies = {
				...policies,
				[name]: {
					permissions: [{ type: 'any', any: true }],
					principals: [{ type: 'any', any: true }]
				}
			};
			newPolicyName = '';
			updateParent();
		}
	}

	function removePolicy(name: string) {
		const { [name]: _, ...rest } = policies;
		policies = rest;
		updateParent();
	}

	function addShadowPolicy() {
		const name = newShadowPolicyName.trim();
		if (name && !(name in shadowPolicies)) {
			shadowPolicies = {
				...shadowPolicies,
				[name]: {
					permissions: [{ type: 'any', any: true }],
					principals: [{ type: 'any', any: true }]
				}
			};
			newShadowPolicyName = '';
			updateParent();
		}
	}

	function removeShadowPolicy(name: string) {
		const { [name]: _, ...rest } = shadowPolicies;
		shadowPolicies = rest;
		updateParent();
	}

	function updatePermissionType(policyName: string, index: number, newType: string, isShadow: boolean) {
		const target = isShadow ? shadowPolicies : policies;
		const policy = target[policyName];
		if (!policy) return;

		const perm = { ...policy.permissions[index] };
		perm.type = newType;
		// Reset type-specific fields
		delete perm.any;
		delete perm.name;
		delete perm.exact_match;
		delete perm.path;
		delete perm.port;

		if (newType === 'any') perm.any = true;

		const newPerms = [...policy.permissions];
		newPerms[index] = perm;

		if (isShadow) {
			shadowPolicies = { ...shadowPolicies, [policyName]: { ...policy, permissions: newPerms } };
		} else {
			policies = { ...policies, [policyName]: { ...policy, permissions: newPerms } };
		}
		updateParent();
	}

	function updatePermissionField(policyName: string, index: number, field: string, value: string | number | boolean, isShadow: boolean) {
		const target = isShadow ? shadowPolicies : policies;
		const policy = target[policyName];
		if (!policy) return;

		const perm = { ...policy.permissions[index], [field]: value };
		const newPerms = [...policy.permissions];
		newPerms[index] = perm;

		if (isShadow) {
			shadowPolicies = { ...shadowPolicies, [policyName]: { ...policy, permissions: newPerms } };
		} else {
			policies = { ...policies, [policyName]: { ...policy, permissions: newPerms } };
		}
		updateParent();
	}

	function addPermission(policyName: string, isShadow: boolean) {
		const target = isShadow ? shadowPolicies : policies;
		const policy = target[policyName];
		if (!policy) return;

		const newPerms = [...policy.permissions, { type: 'any', any: true }];
		if (isShadow) {
			shadowPolicies = { ...shadowPolicies, [policyName]: { ...policy, permissions: newPerms } };
		} else {
			policies = { ...policies, [policyName]: { ...policy, permissions: newPerms } };
		}
		updateParent();
	}

	function removePermission(policyName: string, index: number, isShadow: boolean) {
		const target = isShadow ? shadowPolicies : policies;
		const policy = target[policyName];
		if (!policy || policy.permissions.length <= 1) return;

		const newPerms = policy.permissions.filter((_, i) => i !== index);
		if (isShadow) {
			shadowPolicies = { ...shadowPolicies, [policyName]: { ...policy, permissions: newPerms } };
		} else {
			policies = { ...policies, [policyName]: { ...policy, permissions: newPerms } };
		}
		updateParent();
	}

	function updatePrincipalType(policyName: string, index: number, newType: string, isShadow: boolean) {
		const target = isShadow ? shadowPolicies : policies;
		const policy = target[policyName];
		if (!policy) return;

		const princ = { ...policy.principals[index] };
		princ.type = newType;
		delete princ.any;
		delete princ.principal_name;
		delete princ.address_prefix;
		delete princ.prefix_len;
		delete princ.name;
		delete princ.exact_match;

		if (newType === 'any') princ.any = true;

		const newPrincs = [...policy.principals];
		newPrincs[index] = princ;

		if (isShadow) {
			shadowPolicies = { ...shadowPolicies, [policyName]: { ...policy, principals: newPrincs } };
		} else {
			policies = { ...policies, [policyName]: { ...policy, principals: newPrincs } };
		}
		updateParent();
	}

	function updatePrincipalField(policyName: string, index: number, field: string, value: string | number | boolean, isShadow: boolean) {
		const target = isShadow ? shadowPolicies : policies;
		const policy = target[policyName];
		if (!policy) return;

		const princ = { ...policy.principals[index], [field]: value };
		const newPrincs = [...policy.principals];
		newPrincs[index] = princ;

		if (isShadow) {
			shadowPolicies = { ...shadowPolicies, [policyName]: { ...policy, principals: newPrincs } };
		} else {
			policies = { ...policies, [policyName]: { ...policy, principals: newPrincs } };
		}
		updateParent();
	}

	function addPrincipal(policyName: string, isShadow: boolean) {
		const target = isShadow ? shadowPolicies : policies;
		const policy = target[policyName];
		if (!policy) return;

		const newPrincs = [...policy.principals, { type: 'any', any: true }];
		if (isShadow) {
			shadowPolicies = { ...shadowPolicies, [policyName]: { ...policy, principals: newPrincs } };
		} else {
			policies = { ...policies, [policyName]: { ...policy, principals: newPrincs } };
		}
		updateParent();
	}

	function removePrincipal(policyName: string, index: number, isShadow: boolean) {
		const target = isShadow ? shadowPolicies : policies;
		const policy = target[policyName];
		if (!policy || policy.principals.length <= 1) return;

		const newPrincs = policy.principals.filter((_, i) => i !== index);
		if (isShadow) {
			shadowPolicies = { ...shadowPolicies, [policyName]: { ...policy, principals: newPrincs } };
		} else {
			policies = { ...policies, [policyName]: { ...policy, principals: newPrincs } };
		}
		updateParent();
	}

	function handlePolicyKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			addPolicy();
		}
	}

	function handleShadowPolicyKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			addShadowPolicy();
		}
	}
</script>

{#snippet policyEditor(policyEntries: [string, RbacPolicy][], isShadow: boolean)}
	{#each policyEntries as [policyName, policy]}
		<div class="border border-gray-200 rounded-lg p-4">
			<div class="flex items-center justify-between mb-3">
				<h4 class="text-sm font-medium text-gray-900">{policyName}</h4>
				<button
					type="button"
					onclick={() => isShadow ? removeShadowPolicy(policyName) : removePolicy(policyName)}
					class="text-red-500 hover:text-red-700 p-1"
				>
					<Trash2 class="w-4 h-4" />
				</button>
			</div>

			<!-- Permissions -->
			<div class="mb-3">
				<div class="flex items-center justify-between mb-2">
					<label class="text-xs font-medium text-gray-600 uppercase tracking-wide">Permissions (what)</label>
					<button
						type="button"
						onclick={() => addPermission(policyName, isShadow)}
						class="text-blue-600 hover:text-blue-800 p-0.5"
					>
						<Plus class="w-3.5 h-3.5" />
					</button>
				</div>
				{#each policy.permissions as perm, pi}
					<div class="flex items-start gap-2 mb-2 pl-2 border-l-2 border-blue-200">
						<select
							value={perm.type}
							onchange={(e) => updatePermissionType(policyName, pi, e.currentTarget.value, isShadow)}
							class="w-40 px-2 py-1.5 text-xs border border-gray-300 rounded-md focus:outline-none focus:ring-1 focus:ring-blue-500"
						>
							{#each PERMISSION_TYPES as pt}
								<option value={pt.value}>{pt.label}</option>
							{/each}
						</select>
						{#if perm.type === 'header'}
							<input
								type="text"
								value={perm.name ?? ''}
								oninput={(e) => updatePermissionField(policyName, pi, 'name', e.currentTarget.value, isShadow)}
								placeholder="Header name"
								class="w-32 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
							<input
								type="text"
								value={perm.exact_match ?? ''}
								oninput={(e) => updatePermissionField(policyName, pi, 'exact_match', e.currentTarget.value, isShadow)}
								placeholder="Exact match"
								class="flex-1 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
						{:else if perm.type === 'url_path'}
							<input
								type="text"
								value={perm.path ?? ''}
								oninput={(e) => updatePermissionField(policyName, pi, 'path', e.currentTarget.value, isShadow)}
								placeholder="/api/v1/..."
								class="flex-1 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
						{:else if perm.type === 'destination_port'}
							<input
								type="number"
								value={perm.port ?? ''}
								oninput={(e) => updatePermissionField(policyName, pi, 'port', parseInt(e.currentTarget.value) || 0, isShadow)}
								placeholder="Port"
								min="1"
								max="65535"
								class="w-24 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
						{/if}
						{#if policy.permissions.length > 1}
							<button
								type="button"
								onclick={() => removePermission(policyName, pi, isShadow)}
								class="text-gray-400 hover:text-red-500 p-0.5"
							>
								<Trash2 class="w-3.5 h-3.5" />
							</button>
						{/if}
					</div>
				{/each}
			</div>

			<!-- Principals -->
			<div>
				<div class="flex items-center justify-between mb-2">
					<label class="text-xs font-medium text-gray-600 uppercase tracking-wide">Principals (who)</label>
					<button
						type="button"
						onclick={() => addPrincipal(policyName, isShadow)}
						class="text-blue-600 hover:text-blue-800 p-0.5"
					>
						<Plus class="w-3.5 h-3.5" />
					</button>
				</div>
				{#each policy.principals as princ, pri}
					<div class="flex items-start gap-2 mb-2 pl-2 border-l-2 border-green-200">
						<select
							value={princ.type}
							onchange={(e) => updatePrincipalType(policyName, pri, e.currentTarget.value, isShadow)}
							class="w-40 px-2 py-1.5 text-xs border border-gray-300 rounded-md focus:outline-none focus:ring-1 focus:ring-blue-500"
						>
							{#each PRINCIPAL_TYPES as pt}
								<option value={pt.value}>{pt.label}</option>
							{/each}
						</select>
						{#if princ.type === 'authenticated'}
							<input
								type="text"
								value={princ.principal_name ?? ''}
								oninput={(e) => updatePrincipalField(policyName, pri, 'principal_name', e.currentTarget.value, isShadow)}
								placeholder="Principal name (optional)"
								class="flex-1 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
						{:else if princ.type === 'source_ip' || princ.type === 'direct_remote_ip'}
							<input
								type="text"
								value={princ.address_prefix ?? ''}
								oninput={(e) => updatePrincipalField(policyName, pri, 'address_prefix', e.currentTarget.value, isShadow)}
								placeholder="10.0.0.0"
								class="w-32 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
							<input
								type="number"
								value={princ.prefix_len ?? 32}
								oninput={(e) => updatePrincipalField(policyName, pri, 'prefix_len', parseInt(e.currentTarget.value) || 0, isShadow)}
								placeholder="Prefix len"
								min="0"
								max="128"
								class="w-20 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
						{:else if princ.type === 'header'}
							<input
								type="text"
								value={princ.name ?? ''}
								oninput={(e) => updatePrincipalField(policyName, pri, 'name', e.currentTarget.value, isShadow)}
								placeholder="Header name"
								class="w-32 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
							<input
								type="text"
								value={princ.exact_match ?? ''}
								oninput={(e) => updatePrincipalField(policyName, pri, 'exact_match', e.currentTarget.value, isShadow)}
								placeholder="Exact match"
								class="flex-1 px-2 py-1.5 text-xs border border-gray-300 rounded-md"
							/>
						{/if}
						{#if policy.principals.length > 1}
							<button
								type="button"
								onclick={() => removePrincipal(policyName, pri, isShadow)}
								class="text-gray-400 hover:text-red-500 p-0.5"
							>
								<Trash2 class="w-3.5 h-3.5" />
							</button>
						{/if}
					</div>
				{/each}
			</div>
		</div>
	{/each}
{/snippet}

<div class="space-y-6">
	<!-- Info Box -->
	<div class="rounded-lg border border-blue-100 bg-blue-50 p-3">
		<div class="flex items-start gap-2">
			<Info class="h-5 w-5 text-blue-600 mt-0.5 flex-shrink-0" />
			<div class="text-xs text-blue-700">
				<p class="font-medium">Role-Based Access Control (RBAC)</p>
				<p class="mt-1">
					Enforces access control based on policies with permissions (what actions) and
					principals (who). Each policy defines a set of permissions and principals that
					are combined with the selected action (allow/deny).
				</p>
			</div>
		</div>
	</div>

	<!-- Validation Errors -->
	{#if validationErrors.length > 0}
		<div class="rounded-lg border border-red-200 bg-red-50 p-3">
			<ul class="text-xs text-red-700 list-disc list-inside space-y-0.5">
				{#each validationErrors as err}
					<li>{err}</li>
				{/each}
			</ul>
		</div>
	{/if}

	<!-- Rules Action -->
	<div class="border border-gray-200 rounded-lg p-4">
		<h3 class="text-sm font-medium text-gray-900 mb-3">Rules</h3>
		<div class="space-y-4">
			<div>
				<label class="block text-sm font-medium text-gray-700 mb-1">Action</label>
				<select
					bind:value={rulesAction}
					onchange={updateParent}
					class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				>
					{#each RBAC_ACTIONS as action}
						<option value={action.value}>{action.label} - {action.description}</option>
					{/each}
				</select>
			</div>

			<!-- Policies -->
			<div>
				<h4 class="text-sm font-medium text-gray-700 mb-2">Policies</h4>
				<div class="flex items-center gap-2 mb-3">
					<input
						type="text"
						bind:value={newPolicyName}
						onkeydown={handlePolicyKeydown}
						placeholder="Policy name"
						class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
					<button
						type="button"
						onclick={addPolicy}
						class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
					>
						<Plus class="w-4 h-4" />
					</button>
				</div>

				<div class="space-y-3">
					{@render policyEditor(Object.entries(policies), false)}
				</div>

				{#if Object.keys(policies).length === 0}
					<p class="text-xs text-gray-500 italic">
						No policies defined. Add a policy to define access rules.
					</p>
				{/if}
			</div>
		</div>
	</div>

	<!-- Advanced Settings -->
	<div>
		<button
			type="button"
			onclick={() => (showAdvanced = !showAdvanced)}
			class="flex items-center gap-2 text-sm font-medium text-gray-600 hover:text-gray-900"
		>
			<Settings class="w-4 h-4" />
			<ChevronRight class="w-4 h-4 transition-transform {showAdvanced ? 'rotate-90' : ''}" />
			Advanced Settings
		</button>

		{#if showAdvanced}
			<div class="mt-4 space-y-4 pl-6 border-l-2 border-gray-200">
				<div>
					<label class="block text-sm font-medium text-gray-700 mb-1">
						Rules Stat Prefix
					</label>
					<input
						type="text"
						bind:value={rulesStatPrefix}
						oninput={updateParent}
						placeholder="Optional metrics prefix"
						class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
					/>
				</div>

				<label class="flex items-center gap-3">
					<input
						type="checkbox"
						bind:checked={trackPerRuleStats}
						onchange={updateParent}
						class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
					/>
					<div>
						<span class="text-sm font-medium text-gray-700">Track Per-Rule Statistics</span>
						<p class="text-xs text-gray-500">Track stats for each individual rule</p>
					</div>
				</label>

				<!-- Shadow Rules -->
				<div class="pt-2 border-t border-gray-200">
					<label class="flex items-center gap-3 mb-3">
						<input
							type="checkbox"
							bind:checked={enableShadowRules}
							onchange={updateParent}
							class="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
						/>
						<div>
							<span class="text-sm font-medium text-gray-700">Enable Shadow Rules</span>
							<p class="text-xs text-gray-500">Test rules without enforcement (logged only)</p>
						</div>
					</label>

					{#if enableShadowRules}
						<div class="space-y-3 ml-7">
							<div>
								<label class="block text-sm font-medium text-gray-700 mb-1">Shadow Action</label>
								<select
									bind:value={shadowAction}
									onchange={updateParent}
									class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								>
									{#each RBAC_ACTIONS as action}
										<option value={action.value}>{action.label}</option>
									{/each}
								</select>
							</div>

							<div>
								<label class="block text-sm font-medium text-gray-700 mb-1">
									Shadow Stat Prefix
								</label>
								<input
									type="text"
									bind:value={shadowStatPrefix}
									oninput={updateParent}
									placeholder="Optional"
									class="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
								/>
							</div>

							<div>
								<h4 class="text-sm font-medium text-gray-700 mb-2">Shadow Policies</h4>
								<div class="flex items-center gap-2 mb-3">
									<input
										type="text"
										bind:value={newShadowPolicyName}
										onkeydown={handleShadowPolicyKeydown}
										placeholder="Shadow policy name"
										class="flex-1 px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
									/>
									<button
										type="button"
										onclick={addShadowPolicy}
										class="p-2 text-blue-600 hover:bg-blue-50 rounded-md transition-colors"
									>
										<Plus class="w-4 h-4" />
									</button>
								</div>
								<div class="space-y-3">
									{@render policyEditor(Object.entries(shadowPolicies), true)}
								</div>
							</div>
						</div>
					{/if}
				</div>
			</div>
		{/if}
	</div>
</div>
