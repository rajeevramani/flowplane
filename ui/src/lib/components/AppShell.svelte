<script lang="ts">
	import { goto } from '$app/navigation';
	import { ChevronDown, LogOut, Lock, User } from 'lucide-svelte';
	import Sidebar from './Sidebar.svelte';
	import type { SessionInfoResponse } from '$lib/api/types';
	import { apiClient } from '$lib/api/client';

	interface ResourceCounts {
		routeConfigs: number;
		clusters: number;
		listeners: number;
		imports: number;
		filters: number;
		secrets?: number;
		dataplanes?: number;
	}

	interface Props {
		sessionInfo: SessionInfoResponse;
		selectedTeam: string;
		availableTeams: string[];
		onTeamChange: (team: string) => void;
		resourceCounts?: ResourceCounts;
		statsEnabled?: boolean;
		children: any;
	}

	let {
		sessionInfo,
		selectedTeam,
		availableTeams,
		onTeamChange,
		resourceCounts,
		statsEnabled = false,
		children
	}: Props = $props();

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
		onTeamChange(team);
	}

	// Close menus when clicking outside
	function closeMenus() {
		showProfileMenu = false;
	}
</script>

<svelte:window onclick={closeMenus} />

<div class="h-screen flex overflow-hidden bg-gray-100">
	<!-- Sidebar -->
	<Sidebar {sessionInfo} {resourceCounts} {statsEnabled} />

	<!-- Main Content Area -->
	<div class="flex-1 flex flex-col overflow-hidden">
		<!-- Top Bar -->
		<header class="bg-white border-b border-gray-200 shadow-sm">
			<div class="px-6 py-3 flex items-center justify-between">
				<!-- Left side: Team selector -->
				<div class="flex items-center gap-4">
					{#if availableTeams.length > 0}
						<div class="flex items-center gap-2">
							<label for="teamSelect" class="text-sm font-medium text-gray-600">Team:</label>
							<select
								id="teamSelect"
								onchange={(e) => handleTeamSelect(e.currentTarget.value)}
								class="min-w-[150px] px-3 py-1.5 text-sm border border-gray-300 rounded-md bg-white focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-blue-500 cursor-pointer"
							>
								{#each availableTeams as team (team)}
									<option value={team} selected={team === selectedTeam}>{team}</option>
								{/each}
							</select>
						</div>
					{/if}
				</div>

				<!-- Right side: User menu -->
				<div class="flex items-center gap-4">
					<!-- Role badge -->
					{#if sessionInfo.isAdmin}
						<span
							class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-purple-100 text-purple-800"
						>
							Admin
						</span>
					{:else}
						<span
							class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800"
						>
							Developer
						</span>
					{/if}

					<!-- Profile Dropdown -->
					<div class="relative">
						<button
							onclick={(e) => {
								e.stopPropagation();
								showProfileMenu = !showProfileMenu;
							}}
							class="flex items-center gap-2 px-3 py-1.5 rounded-md hover:bg-gray-100 transition-colors"
						>
							<div class="h-8 w-8 rounded-full bg-gray-200 flex items-center justify-center">
								<User class="h-4 w-4 text-gray-600" />
							</div>
							<div class="text-left hidden sm:block">
								<div class="text-sm font-medium text-gray-900">{sessionInfo.name}</div>
								<div class="text-xs text-gray-500">{sessionInfo.email}</div>
							</div>
							<ChevronDown class="h-4 w-4 text-gray-500" />
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
										class="flex items-center gap-2 px-4 py-2 text-sm text-gray-700 hover:bg-gray-100"
									>
										<Lock class="h-4 w-4" />
										Change Password
									</a>

									{#if sessionInfo.teams.length > 0}
										<div class="px-4 py-2 border-t border-gray-100">
											<p class="text-xs font-medium text-gray-500 mb-1">Your Teams</p>
											<div class="flex flex-wrap gap-1">
												{#each sessionInfo.teams as team}
													<span
														class="inline-block px-2 py-0.5 text-xs bg-indigo-100 text-indigo-800 rounded"
													>
														{team}
													</span>
												{/each}
											</div>
										</div>
									{/if}

									<div class="border-t border-gray-100">
										<button
											onclick={handleLogout}
											class="flex items-center gap-2 w-full text-left px-4 py-2 text-sm text-gray-700 hover:bg-gray-100"
										>
											<LogOut class="h-4 w-4" />
											Sign out
										</button>
									</div>
								</div>
							</div>
						{/if}
					</div>
				</div>
			</div>
		</header>

		<!-- Main Content -->
		<main class="flex-1 overflow-y-auto p-6">
			<div class="w-full">
				{@render children()}
			</div>
		</main>
	</div>
</div>
