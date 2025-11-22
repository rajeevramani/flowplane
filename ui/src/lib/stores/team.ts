import { writable } from 'svelte/store';

// Create a writable store for the selected team
export const selectedTeam = writable<string>('');

// Initialize from sessionStorage if available (client-side only)
if (typeof window !== 'undefined') {
	const stored = sessionStorage.getItem('selected_team');
	if (stored) {
		selectedTeam.set(stored);
	}
}

// Sync to sessionStorage on change
selectedTeam.subscribe((value) => {
	if (typeof window !== 'undefined' && value) {
		sessionStorage.setItem('selected_team', value);
	}
});

/**
 * Set the selected team and persist to sessionStorage
 */
export function setSelectedTeam(team: string): void {
	selectedTeam.set(team);
}

/**
 * Initialize the selected team from a list of available teams
 * Tries to restore from sessionStorage, falls back to first team
 */
export function initializeSelectedTeam(availableTeams: string[]): string {
	if (typeof window === 'undefined' || availableTeams.length === 0) {
		return '';
	}

	const storedTeam = sessionStorage.getItem('selected_team');
	if (storedTeam && availableTeams.includes(storedTeam)) {
		selectedTeam.set(storedTeam);
		return storedTeam;
	}

	// Fall back to first team
	const firstTeam = availableTeams[0];
	selectedTeam.set(firstTeam);
	return firstTeam;
}
