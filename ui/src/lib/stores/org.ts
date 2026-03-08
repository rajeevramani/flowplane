import { writable } from 'svelte/store';
import type { OrganizationResponse, OrgRole } from '$lib/api/types';

interface OrgContext {
	organization: OrganizationResponse | null;
	role: OrgRole | null;
}

export const currentOrg = writable<OrgContext>({ organization: null, role: null });

export function isOrgAdmin(orgRole?: string): boolean {
	return orgRole === 'admin' || orgRole === 'owner';
}
