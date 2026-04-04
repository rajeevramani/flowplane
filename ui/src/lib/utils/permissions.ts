import type { GrantSummary, SessionInfoResponse } from '$lib/api/types';

/**
 * Check if user is a platform governance admin (can manage orgs, users, audit).
 * Does NOT grant resource access — governance only.
 */
export function isGovernanceAdmin(user: SessionInfoResponse): boolean {
	return user.orgScopes?.includes('admin:all') ?? false;
}

/**
 * Check if a user has a specific grant for a resource+action pair.
 * Checks across all teams — no admin bypass.
 */
export function hasGrant(grants: GrantSummary[], resource: string, action: string): boolean {
	return grants.some((g) => g.resourceType === resource && g.action === action);
}

/**
 * Check if a user has a specific scope (resource:action format).
 * Delegates to hasGrant using the user's grants array.
 */
export function hasScope(user: SessionInfoResponse, requiredScope: string): boolean {
	const [resource, action] = requiredScope.split(':');
	return hasGrant(user.grants, resource, action);
}

/**
 * Check if user can read aggregated schemas.
 */
export function canReadSchemas(user: SessionInfoResponse): boolean {
	return hasGrant(user.grants, 'aggregated-schemas', 'read');
}

/**
 * Check if user can create learning sessions.
 */
export function canCreateLearningSessions(user: SessionInfoResponse): boolean {
	return hasGrant(user.grants, 'learning-sessions', 'create');
}

/**
 * Check if user can delete learning sessions.
 */
export function canDeleteLearningSessions(user: SessionInfoResponse): boolean {
	return hasGrant(user.grants, 'learning-sessions', 'delete');
}

/**
 * Check if user can read learning sessions.
 */
export function canReadLearningSessions(user: SessionInfoResponse): boolean {
	return hasGrant(user.grants, 'learning-sessions', 'read');
}
