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

export interface ChangePasswordRequest {
	currentPassword: string;
	newPassword: string;
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

export interface ListTeamsResponse {
	teams: string[];
}

export type TeamStatus = 'active' | 'suspended' | 'archived';

export interface TeamResponse {
	id: string;
	name: string;
	displayName: string;
	description: string | null;
	ownerUserId: string | null;
	settings: any | null;
	status: TeamStatus;
	createdAt: string;
	updatedAt: string;
}

export interface CreateTeamRequest {
	name: string;
	displayName: string;
	description?: string | null;
	ownerUserId?: string | null;
	settings?: any | null;
}

export interface UpdateTeamRequest {
	displayName?: string;
	description?: string | null;
	ownerUserId?: string | null;
	settings?: any | null;
	status?: TeamStatus;
}

export interface AdminListTeamsResponse {
	teams: TeamResponse[];
	total: number;
	limit: number;
	offset: number;
}

export interface DashboardStats {
	importsCount: number;
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

export interface ImportResponse {
	importId: string;
	specName: string;
	specVersion: string | null;
	routesCreated: number;
	clustersCreated: number;
	clustersReused: number;
	listenerName: string | null;
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

// Import types (replacing API Definition types)
export interface ImportSummary {
	id: string;
	specName: string;
	specVersion: string | null;
	team: string;
	importedAt: string;
	updatedAt: string;
}

export interface ImportDetailsResponse {
	id: string;
	specName: string;
	specVersion: string | null;
	specChecksum: string | null;
	team: string;
	importedAt: string;
	updatedAt: string;
	routeCount: number;
	clusterCount: number;
	listenerCount: number;
}

// Listener types
export interface ListenerResponse {
	name: string;
	team: string;
	address: string;
	port: number | null;
	protocol: string;
	version: number;
	importId?: string;
	config: any; // Full listener config
}

// Route types
export interface RouteResponse {
	name: string;
	team: string;
	pathPrefix: string;
	clusterTargets: string;
	importId?: string;
	routeOrder?: number;
	config: any; // Full route config
}

// Legacy type - no longer used after API definitions removal
// Routes are now accessed directly via RouteResponse

// Cluster types
export interface ClusterResponse {
	name: string;
	team: string;
	serviceName: string;
	importId?: string;
	config: any; // Full cluster config
}

// Bootstrap configuration types
export interface BootstrapConfigRequest {
	team: string;
	format?: 'yaml' | 'json';
}

// User Management types
export type UserStatus = 'Active' | 'Inactive' | 'Suspended';

export interface UserResponse {
	id: string;
	email: string;
	name: string;
	status: UserStatus;
	isAdmin: boolean;
	createdAt: string;
	updatedAt: string;
}

export interface UserTeamMembership {
	id: string;
	userId: string;
	team: string;
	scopes: string[];
	createdAt: string;
}

export interface UserWithTeamsResponse {
	id: string;
	email: string;
	name: string;
	status: UserStatus;
	isAdmin: boolean;
	createdAt: string;
	updatedAt: string;
	teams: UserTeamMembership[];
}

export interface CreateUserRequest {
	email: string;
	password: string;
	name: string;
	isAdmin?: boolean;
}

export interface UpdateUserRequest {
	email?: string;
	name?: string;
	status?: UserStatus;
	isAdmin?: boolean;
}

export interface CreateTeamMembershipRequest {
	userId: string;
	team: string;
	scopes: string[];
}

export interface ListUsersResponse {
	users: UserResponse[];
	total: number;
	limit: number;
	offset: number;
}

// Audit Log Types
export interface AuditLogEntry {
	id: number;
	resource_type: string;
	resource_id: string | null;
	resource_name: string | null;
	action: string;
	old_configuration: string | null;
	new_configuration: string | null;
	user_id: string | null;
	client_ip: string | null;
	user_agent: string | null;
	created_at: string;
}

export interface ListAuditLogsQuery {
	resource_type?: string;
	action?: string;
	user_id?: string;
	start_date?: string;
	end_date?: string;
	limit?: number;
	offset?: number;
}

export interface ListAuditLogsResponse {
	entries: AuditLogEntry[];
	total: number;
	limit: number;
	offset: number;
}
