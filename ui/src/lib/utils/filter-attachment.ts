/**
 * Utility functions for filter attachment validation and display.
 * These functions help ensure filters are attached to compatible resource types.
 */

import type { AttachmentPoint, FilterType } from '$lib/api/types';

/**
 * Mapping of filter types to their allowed attachment points.
 * This mirrors the backend implementation in src/domain/filter.rs
 */
const FILTER_ATTACHMENT_RULES: Record<FilterType, AttachmentPoint[]> = {
	header_mutation: ['route'],
	cors: ['route'],
	jwt_auth: ['route', 'listener'],
	jwt_authn: ['route', 'listener'], // Legacy name from Envoy filter
	local_rate_limit: ['route', 'listener'],
	rate_limit: ['route', 'listener'],
	ext_authz: ['route', 'listener']
};

/**
 * Check if a filter type can attach to routes.
 */
export function canAttachToRoute(filterType: FilterType): boolean {
	return FILTER_ATTACHMENT_RULES[filterType]?.includes('route') ?? false;
}

/**
 * Check if a filter type can attach to listeners.
 */
export function canAttachToListener(filterType: FilterType): boolean {
	return FILTER_ATTACHMENT_RULES[filterType]?.includes('listener') ?? false;
}

/**
 * Check if a filter type can attach to clusters (future).
 */
export function canAttachToCluster(filterType: FilterType): boolean {
	return FILTER_ATTACHMENT_RULES[filterType]?.includes('cluster') ?? false;
}

/**
 * Get all allowed attachment points for a filter type.
 */
export function getAllowedAttachmentPoints(filterType: FilterType): AttachmentPoint[] {
	return FILTER_ATTACHMENT_RULES[filterType] ?? [];
}

/**
 * Get a human-readable label for an attachment point.
 */
export function getAttachmentPointLabel(point: AttachmentPoint): string {
	switch (point) {
		case 'route':
			return 'Routes';
		case 'listener':
			return 'Listeners';
		case 'cluster':
			return 'Clusters';
		default:
			return point;
	}
}

/**
 * Get a display string for allowed attachment points (e.g., "Routes only" or "Routes, Listeners").
 */
export function getAllowedAttachmentPointsDisplay(filterType: FilterType): string {
	const points = getAllowedAttachmentPoints(filterType);
	if (points.length === 0) {
		return 'None';
	}
	if (points.length === 1) {
		return `${getAttachmentPointLabel(points[0])} only`;
	}
	return points.map(getAttachmentPointLabel).join(', ');
}

/**
 * Check if a filter can be attached to a specific attachment point.
 */
export function canAttachTo(filterType: FilterType, point: AttachmentPoint): boolean {
	return FILTER_ATTACHMENT_RULES[filterType]?.includes(point) ?? false;
}

/**
 * Get an error message for why a filter cannot be attached to a resource type.
 */
export function getAttachmentErrorMessage(
	filterType: FilterType,
	attemptedPoint: AttachmentPoint
): string | null {
	if (canAttachTo(filterType, attemptedPoint)) {
		return null;
	}

	const allowedPoints = getAllowedAttachmentPoints(filterType);
	const allowedDisplay =
		allowedPoints.length > 0
			? allowedPoints.map(getAttachmentPointLabel).join(', ')
			: 'no resources';

	return `Filter type '${filterType}' cannot be attached to ${getAttachmentPointLabel(attemptedPoint).toLowerCase()}. Valid attachment points: ${allowedDisplay}`;
}
