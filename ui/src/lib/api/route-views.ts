// Route Views API Client
// Provides flattened route views for the routes list page

import { apiClient } from './client';
import type {
	RouteListResponseDto,
	RouteListStatsDto,
	RouteListQueryParams
} from '$lib/types/route-view';

/**
 * List all route views with pagination and filtering.
 * Returns a flattened view of routes for the UI list display.
 */
export async function listRouteViews(
	query?: RouteListQueryParams
): Promise<RouteListResponseDto> {
	const params = new URLSearchParams();

	if (query?.page !== undefined) {
		params.append('page', String(query.page));
	}
	if (query?.pageSize !== undefined) {
		params.append('pageSize', String(query.pageSize));
	}
	if (query?.search) {
		params.append('search', query.search);
	}
	if (query?.mcpFilter) {
		params.append('mcpFilter', query.mcpFilter);
	}
	if (query?.routeConfig) {
		params.append('routeConfig', query.routeConfig);
	}
	if (query?.virtualHost) {
		params.append('virtualHost', query.virtualHost);
	}

	const queryString = params.toString();
	const path = `/api/v1/route-views${queryString ? `?${queryString}` : ''}`;
	return apiClient.get<RouteListResponseDto>(path);
}

/**
 * Get route view statistics.
 * Returns aggregate stats for the route list page header.
 */
export async function getRouteViewStats(): Promise<RouteListStatsDto> {
	return apiClient.get<RouteListStatsDto>('/api/v1/route-views/stats');
}
