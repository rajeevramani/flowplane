import { writable } from 'svelte/store';
import type { OrganizationResponse, OrgRole } from '$lib/api/types';

interface OrgContext {
	organization: OrganizationResponse | null;
	role: OrgRole | null;
}

export const currentOrg = writable<OrgContext>({ organization: null, role: null });

export function isSystemAdmin(scopes: string[]): boolean {
	return scopes.includes('admin:all');
}

export function isOrgAdmin(scopes: string[]): boolean {
	return scopes.some((s) => /^org:[^:]+:admin$/.test(s));
}

export function hasOrgScope(scopes: string[], orgName: string): boolean {
	return scopes.some((s) => s.startsWith(`org:${orgName}:`));
}
