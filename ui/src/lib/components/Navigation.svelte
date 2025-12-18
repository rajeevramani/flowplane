<script lang="ts">
	import { apiClient } from '$lib/api/client';
	import { goto } from '$app/navigation';
	import type { SessionInfoResponse } from '$lib/api/types';

	interface Props {
		sessionInfo: SessionInfoResponse;
		selectedTeam?: string;
		availableTeams?: string[];
		onTeamChange?: (team: string) => void;
	}

	let { sessionInfo, selectedTeam = '', availableTeams = [], onTeamChange }: Props = $props();

	let showProfileMenu = $state(false);

	async function handleLogout() {
		try {
			await apiClient.logout();
			goto('/login');
		} catch (error) {
			console.error('Logout failed:', error);
			goto('/login');
		}
	}

	function handleTeamSelect(team: string) {
		if (onTeamChange) {
			onTeamChange(team);
		}
	}

	// Close menus when clicking outside
	function closeMenus() {
		showProfileMenu = false;
	}
</script>

<svelte:window onclick={closeMenus} />

<nav class="bg-white shadow-sm border-b border-gray-200">
	<div class="w-full px-4 sm:px-6 lg:px-8">
		<div class="flex justify-between h-16 items-center">
			<!-- Logo and Brand -->
			<div class="flex items-center gap-4">
				<a href="/dashboard" class="flex items-center gap-3">
					<h1 class="text-xl font-bold text-gray-900">Flowplane</h1>
					<span class="text-sm text-gray-500">API Gateway Platform</span>
				</a>
			</div>

			<!-- Main Navigation -->
			<div class="flex items-center gap-1">
				<!-- Dashboard Link -->
				<a
					href="/dashboard"
					class="px-3 py-2 text-sm font-medium text-gray-700 hover:text-gray-900 hover:bg-gray-100 rounded-md transition-colors"
				>
					Dashboard
				</a>

				<!-- Admin Links (only for admins) -->
				{#if sessionInfo.isAdmin}
					<a
						href="/admin/users"
						class="px-3 py-2 text-sm font-medium text-gray-700 hover:text-gray-900 hover:bg-gray-100 rounded-md transition-colors"
					>
						Users
					</a>
					<a
						href="/admin/teams"
						class="px-3 py-2 text-sm font-medium text-gray-700 hover:text-gray-900 hover:bg-gray-100 rounded-md transition-colors"
					>
						Teams
					</a>
				{/if}

				<!-- Resources Link -->
				<a
					href="/resources"
					class="px-3 py-2 text-sm font-medium text-gray-700 hover:text-gray-900 hover:bg-gray-100 rounded-md transition-colors"
				>
					Resources
				</a>

				<!-- PAT Link -->
				<a
					href="/tokens"
					class="px-3 py-2 text-sm font-medium text-gray-700 hover:text-gray-900 hover:bg-gray-100 rounded-md transition-colors"
				>
					PAT
				</a>
			</div>

			<!-- Right Side: Team Selector & Profile -->
			<div class="flex items-center gap-4">
				<!-- Team Switcher (only for non-admin with multiple teams) -->
				{#if !sessionInfo.isAdmin && availableTeams.length > 1}
					<div class="flex items-center gap-2">
						<label for="teamSelect" class="text-sm text-gray-600">Team:</label>
						<select
							id="teamSelect"
							value={selectedTeam}
							onchange={(e) => handleTeamSelect(e.currentTarget.value)}
							class="px-3 py-1.5 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
						>
							{#each availableTeams as team}
								<option value={team}>{team}</option>
							{/each}
						</select>
					</div>
				{/if}

				<!-- Profile Dropdown -->
				<div class="relative">
					<button
						onclick={(e) => {
							e.stopPropagation();
							showProfileMenu = !showProfileMenu;
						}}
						class="flex items-center gap-2 px-3 py-2 rounded-md hover:bg-gray-100 transition-colors"
					>
						<div class="text-right">
							<div class="text-sm font-medium text-gray-900">{sessionInfo.name}</div>
							<div class="text-xs text-gray-500">{sessionInfo.email}</div>
						</div>
						{#if sessionInfo.isAdmin}
							<span
								class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-purple-100 text-purple-800"
								title="Administrator - Full system access"
							>
								Admin
							</span>
						{:else}
							<span
								class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800"
								title="Developer - Team-scoped access"
							>
								Developer
							</span>
						{/if}
						<svg class="h-4 w-4 text-gray-500" fill="none" viewBox="0 0 24 24" stroke="currentColor">
							<path
								stroke-linecap="round"
								stroke-linejoin="round"
								stroke-width="2"
								d="M19 9l-7 7-7-7"
							/>
						</svg>
					</button>

					{#if showProfileMenu}
						<div
							class="absolute right-0 mt-2 w-56 bg-white rounded-md shadow-lg border border-gray-200 z-50"
							onclick={(e) => e.stopPropagation()}
						>
							<div class="py-1">
								<div class="px-4 py-2 border-b border-gray-100">
									<p class="text-sm font-medium text-gray-900">{sessionInfo.name}</p>
									<p class="text-xs text-gray-500">{sessionInfo.email}</p>
								</div>
								<a
									href="/profile/password"
									class="block px-4 py-2 text-sm text-gray-700 hover:bg-gray-100"
								>
									Change Password
								</a>
								{#if sessionInfo.teams.length > 0}
									<div class="px-4 py-2 border-t border-gray-100">
										<p class="text-xs font-medium text-gray-500 mb-1">Your Teams</p>
										{#each sessionInfo.teams as team}
											<span
												class="inline-block mr-1 mb-1 px-2 py-0.5 text-xs bg-indigo-100 text-indigo-800 rounded"
											>
												{team}
											</span>
										{/each}
									</div>
								{/if}
								<div class="border-t border-gray-100"></div>
								<button
									onclick={handleLogout}
									class="block w-full text-left px-4 py-2 text-sm text-gray-700 hover:bg-gray-100"
								>
									Sign out
								</button>
							</div>
						</div>
					{/if}
				</div>
			</div>
		</div>
	</div>
</nav>
