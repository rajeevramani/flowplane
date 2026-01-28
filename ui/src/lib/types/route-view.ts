// Route View Types - Matching backend DTOs from src/api/dto/route_view.rs
// These types support the flat table view for routes list (Option A prototype)

/**
 * Route match type from backend RouteMatchType enum
 */
export type RouteMatchType = 'prefix' | 'exact' | 'regex' | 'path_template' | 'connect_matcher';

/**
 * Flattened route view for UI list display.
 * All fields are derived from existing data at runtime - no schema changes required.
 */
export interface RouteListViewDto {
	// === Identity (from normalized tables) ===
	/** Unique ID of the route */
	routeId: string;
	/** Name of the route */
	routeName: string;
	/** ID of the parent virtual host */
	virtualHostId: string;
	/** Name of the parent virtual host */
	virtualHostName: string;
	/** ID of the parent route configuration */
	routeConfigId: string;
	/** Name of the parent route configuration */
	routeConfigName: string;
	/** Team that owns this route's configuration */
	team: string | null;

	// === From routes table (already denormalized) ===
	/** Path pattern for matching (e.g., "/api/users", "/v1/.*") */
	pathPattern: string;
	/** Type of path matching (prefix, exact, regex, path_template) */
	matchType: RouteMatchType;
	/** Order of the route within the virtual host (affects matching priority) */
	ruleOrder: number;

	// === From virtual_hosts table ===
	/** Domains the virtual host matches */
	domains: string[];

	// === Derived from configuration JSON at runtime ===
	/** Primary upstream cluster for traffic routing */
	upstreamCluster: string | null;
	/** Fallback cluster if primary is unavailable */
	fallbackCluster: string | null;
	/** HTTP methods this route handles (empty = all methods) */
	httpMethods: string[];
	/** Request timeout in seconds */
	timeoutSeconds: number | null;
	/** Path prefix rewrite rule */
	prefixRewrite: string | null;

	// === From related tables (JOINs) ===
	/** Whether MCP tool is enabled for this route */
	mcpEnabled: boolean;
	/** Name of the MCP tool (if enabled) */
	mcpToolName: string | null;
	/** Number of filters attached to this route */
	filterCount: number;

	// === From route_metadata table ===
	/** OpenAPI operation ID (if imported from OpenAPI spec) */
	operationId: string | null;
	/** Summary description of the route */
	summary: string | null;

	// === Timestamps ===
	/** When the route was created (ISO 8601) */
	createdAt: string;
	/** When the route was last updated (ISO 8601) */
	updatedAt: string;
}

/**
 * Statistics summary for the route list page.
 * All values are computed on-the-fly from existing tables.
 */
export interface RouteListStatsDto {
	/** Total number of routes */
	totalRoutes: number;
	/** Total number of virtual hosts */
	totalVirtualHosts: number;
	/** Total number of route configurations */
	totalRouteConfigs: number;
	/** Number of routes with MCP enabled */
	mcpEnabledCount: number;
	/** Number of unique upstream clusters */
	uniqueClusters: number;
	/** Number of unique domains */
	uniqueDomains: number;
}

/**
 * Pagination metadata for list responses.
 */
export interface PaginationDto {
	/** Current page number (1-indexed) */
	page: number;
	/** Number of items per page */
	pageSize: number;
	/** Total number of items across all pages */
	totalCount: number;
	/** Total number of pages */
	totalPages: number;
}

/**
 * Paginated response for route list endpoint.
 */
export interface RouteListResponseDto {
	/** List of routes for the current page */
	items: RouteListViewDto[];
	/** Aggregate statistics */
	stats: RouteListStatsDto;
	/** Pagination information */
	pagination: PaginationDto;
}

/**
 * Query parameters for route list endpoint.
 */
export interface RouteListQueryParams {
	/** Page number (1-indexed, default: 1) */
	page?: number;
	/** Number of items per page (default: 20, max: 100) */
	pageSize?: number;
	/** Search query (searches name, path, domain, cluster) */
	search?: string;
	/** Filter by MCP status ("enabled", "disabled", or undefined for all) */
	mcpFilter?: 'enabled' | 'disabled';
	/** Filter by route config name */
	routeConfig?: string;
	/** Filter by virtual host name */
	virtualHost?: string;
}
