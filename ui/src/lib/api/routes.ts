// Single Route API Client
// Provides operations for individual routes within route configurations

import { apiClient } from './client';
import type {
	RouteResponse,
	UpdateRouteBody,
	VirtualHostDefinition,
	RouteRuleDefinition,
	FilterResponse,
	McpStatus,
	EnableMcpRequest
} from './types';
import type { RouteListViewDto } from '$lib/types/route-view';

/**
 * Extended route data for editing a single route.
 * Combines the flat view DTO with the full route config structure.
 */
export interface SingleRouteEditData {
	/** Flat view data for display */
	view: RouteListViewDto;
	/** Full route configuration */
	config: RouteResponse;
	/** The specific virtual host containing this route */
	virtualHost: VirtualHostDefinition;
	/** The specific route rule within the virtual host */
	route: RouteRuleDefinition;
	/** Index of the virtual host in the config */
	virtualHostIndex: number;
	/** Index of the route within the virtual host */
	routeIndex: number;
}

/**
 * Get a single route by ID for editing.
 * This loads the route view data and extracts the specific route from the full config.
 */
export async function getSingleRouteForEdit(routeId: string): Promise<SingleRouteEditData> {
	// First, get the route view to find the config name
	const routeViewsResponse = await apiClient.get<{
		items: RouteListViewDto[];
		stats: unknown;
		pagination: unknown;
	}>('/api/v1/route-views');

	const routeView = routeViewsResponse.items.find((r) => r.routeId === routeId);
	if (!routeView) {
		throw new Error(`Route with ID ${routeId} not found`);
	}

	// Load the full route config
	const config = await apiClient.getRouteConfig(routeView.routeConfigName);

	// Find the virtual host and route within the config
	let virtualHost: VirtualHostDefinition | null = null;
	let route: RouteRuleDefinition | null = null;
	let virtualHostIndex = -1;
	let routeIndex = -1;

	for (let vhIdx = 0; vhIdx < config.config.virtualHosts.length; vhIdx++) {
		const vh = config.config.virtualHosts[vhIdx];
		if (vh.name === routeView.virtualHostName) {
			virtualHost = vh;
			virtualHostIndex = vhIdx;

			for (let rIdx = 0; rIdx < vh.routes.length; rIdx++) {
				const r = vh.routes[rIdx];
				if (r.name === routeView.routeName) {
					route = r;
					routeIndex = rIdx;
					break;
				}
			}
			break;
		}
	}

	if (!virtualHost || !route) {
		throw new Error(`Could not find route ${routeView.routeName} in config ${routeView.routeConfigName}`);
	}

	return {
		view: routeView,
		config,
		virtualHost,
		route,
		virtualHostIndex,
		routeIndex
	};
}

/**
 * Update a single route within a route configuration.
 * This modifies only the specified route and preserves all other routes.
 */
export async function updateSingleRoute(
	configName: string,
	virtualHostIndex: number,
	routeIndex: number,
	updatedRoute: RouteRuleDefinition
): Promise<RouteResponse> {
	// First, get the current full config
	const currentConfig = await apiClient.getRouteConfig(configName);

	// Clone the config to avoid mutation
	const updatedConfig: UpdateRouteBody = {
		team: currentConfig.team,
		name: currentConfig.name,
		virtualHosts: JSON.parse(JSON.stringify(currentConfig.config.virtualHosts))
	};

	// Update the specific route
	updatedConfig.virtualHosts[virtualHostIndex].routes[routeIndex] = updatedRoute;

	// Send the update request
	return apiClient.updateRouteConfig(configName, updatedConfig);
}

/**
 * Delete a single route from a route configuration.
 * This removes only the specified route and preserves all other routes.
 */
export async function deleteSingleRoute(
	configName: string,
	virtualHostIndex: number,
	routeIndex: number
): Promise<RouteResponse> {
	// Get the current full config
	const currentConfig = await apiClient.getRouteConfig(configName);

	// Clone the config
	const updatedConfig: UpdateRouteBody = {
		team: currentConfig.team,
		name: currentConfig.name,
		virtualHosts: JSON.parse(JSON.stringify(currentConfig.config.virtualHosts))
	};

	// Remove the specific route
	updatedConfig.virtualHosts[virtualHostIndex].routes.splice(routeIndex, 1);

	// Send the update request
	return apiClient.updateRouteConfig(configName, updatedConfig);
}

/**
 * Get MCP status for a specific route.
 */
export async function getMcpStatusForRoute(
	team: string,
	routeId: string
): Promise<McpStatus> {
	return apiClient.getMcpStatus(team, routeId);
}

/**
 * Enable MCP for a specific route.
 */
export async function enableMcpForRoute(
	team: string,
	routeId: string,
	request?: EnableMcpRequest
): Promise<void> {
	await apiClient.enableMcp(team, routeId, request);
}

/**
 * Disable MCP for a specific route.
 */
export async function disableMcpForRoute(
	team: string,
	routeId: string
): Promise<void> {
	await apiClient.disableMcp(team, routeId);
}

/**
 * Get filters attached to a specific route.
 */
export async function getRouteFilters(
	configName: string,
	virtualHostName: string,
	routeName: string
): Promise<FilterResponse[]> {
	const response = await apiClient.get<{ filters: FilterResponse[] }>(
		`/api/v1/route-configs/${configName}/virtual-hosts/${virtualHostName}/routes/${routeName}/filters`
	);
	return response.filters;
}
