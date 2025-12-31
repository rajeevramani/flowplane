import type { SessionInfoResponse } from '$lib/api/types';

/**
 * Check if a user has a specific scope.
 * Admins automatically have all scopes.
 * Supports:
 * - Exact match: "aggregated-schemas:read"
 * - Team-scoped: "team:engineering:aggregated-schemas:read" matches "aggregated-schemas:read"
 * - Wildcard: "learning-sessions:*" matches "learning-sessions:read"
 * - Team wildcard: "team:engineering:*:*" matches any resource for that team
 */
export function hasScope(user: SessionInfoResponse, requiredScope: string): boolean {
	if (user.isAdmin) return true;

	// Parse the required scope (format: "resource:action")
	const [requiredResource, requiredAction] = requiredScope.split(':');

	return user.scopes.some((scope) => {
		// Exact match (global scope)
		if (scope === requiredScope) return true;

		// Team-scoped match: "team:{team}:{resource}:{action}"
		const parts = scope.split(':');
		if (parts.length === 4 && parts[0] === 'team') {
			const [, , resource, action] = parts;
			// Check if team scope matches required resource:action
			if (resource === requiredResource && action === requiredAction) {
				return true;
			}
			// Check team wildcard: team:{team}:*:*
			if (resource === '*' && action === '*') {
				return true;
			}
			// Check action wildcard: team:{team}:{resource}:*
			if (resource === requiredResource && action === '*') {
				return true;
			}
		}

		// Global wildcard: "resource:*" matches "resource:action"
		if (scope.endsWith(':*')) {
			const prefix = scope.slice(0, -1);
			return requiredScope.startsWith(prefix);
		}

		return false;
	});
}

/**
 * Check if user can read aggregated schemas.
 */
export function canReadSchemas(user: SessionInfoResponse): boolean {
	return hasScope(user, 'aggregated-schemas:read');
}

/**
 * Check if user can write (create/update) learning sessions.
 */
export function canWriteLearningSessions(user: SessionInfoResponse): boolean {
	return hasScope(user, 'learning-sessions:write');
}

/**
 * Check if user can delete learning sessions.
 */
export function canDeleteLearningSessions(user: SessionInfoResponse): boolean {
	return hasScope(user, 'learning-sessions:delete');
}

/**
 * Check if user can read learning sessions.
 */
export function canReadLearningSessions(user: SessionInfoResponse): boolean {
	return hasScope(user, 'learning-sessions:read');
}
