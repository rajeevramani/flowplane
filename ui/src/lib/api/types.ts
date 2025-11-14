// API response types matching backend DTOs

export interface LoginRequest {
	email: string;
	password: string;
}

export interface LoginResponse {
	sessionId: string;
	csrfToken: string;
	expiresAt: string;
	userId: string;
	userEmail: string;
	teams: string[];
	scopes: string[];
}

export interface BootstrapStatusResponse {
	needsInitialization: boolean;
	message: string;
}

export interface BootstrapInitializeRequest {
	email: string;
	password: string;
	name: string;
}

export interface BootstrapInitializeResponse {
	setupToken: string;
	expiresAt: string;
	maxUsageCount: number;
	message: string;
	nextSteps: string[];
}

export interface SessionInfoResponse {
	sessionId: string;
	userId: string;
	name: string;
	email: string;
	isAdmin: boolean;
	teams: string[];
	scopes: string[];
	expiresAt: string | null;
}

export interface DashboardStats {
	apiDefinitionsCount: number;
	listenersCount: number;
	routesCount: number;
	clustersCount: number;
}

export interface ApiError {
	message: string;
	code?: string;
}

export type TokenStatus = 'Active' | 'Revoked' | 'Expired';

export interface PersonalAccessToken {
	id: string;
	name: string;
	description: string | null;
	status: TokenStatus;
	expiresAt: string | null;
	lastUsedAt: string | null;
	createdBy: string | null;
	createdAt: string;
	updatedAt: string;
	scopes: string[];
}

export interface CreateTokenRequest {
	name: string;
	description?: string;
	expiresAt?: string | null;
	scopes: string[];
}

export interface TokenSecretResponse {
	id: string;
	token: string;
}

export interface UpdateTokenRequest {
	name?: string;
	description?: string;
}

export interface ImportOpenApiRequest {
	spec: string; // YAML or JSON string
	team?: string;
	listenerIsolation?: boolean;
	port?: number;
}

export interface CreateApiDefinitionResponse {
	id: string;
	bootstrapUri: string;
	routes: string[];
}

export interface OpenApiSpec {
	openapi?: string;
	swagger?: string;
	info: {
		title: string;
		version: string;
		description?: string;
	};
	servers?: Array<{
		url: string;
		description?: string;
	}>;
	paths: Record<string, any>;
}

// API Definition types
export interface ApiDefinitionSummary {
	id: string;
	team: string;
	domain: string;
	listenerIsolation: boolean;
	bootstrapUri: string | null;
	version: number;
	createdAt: string;
	updatedAt: string;
}

// Listener types
export interface ListenerResponse {
	name: string;
	address: string;
	port: number | null;
	protocol: string;
	version: number;
	config: any; // Full listener config
}

// Route types
export interface RouteResponse {
	name: string;
	pathPrefix: string;
	clusterTargets: string;
	config: any; // Full route config
}

// Cluster types
export interface ClusterResponse {
	name: string;
	serviceName: string;
	config: any; // Full cluster config
}

// Bootstrap configuration types
export interface BootstrapConfigRequest {
	team: string;
	format?: 'yaml' | 'json';
	includeDefault?: boolean;
}
