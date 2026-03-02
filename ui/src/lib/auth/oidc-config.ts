import { UserManager, WebStorageStateStore } from 'oidc-client-ts';
import { env } from '$env/dynamic/public';

interface AuthConfig {
	issuer: string;
	client_id: string;
	app_url: string;
}

let cachedManager: UserManager | null = null;
let configPromise: Promise<AuthConfig> | null = null;

async function fetchAuthConfig(): Promise<AuthConfig> {
	const apiBase = env.PUBLIC_API_BASE || '';
	const response = await fetch(`${apiBase}/api/v1/auth/config`);
	if (!response.ok) {
		throw new Error(`Failed to fetch auth config: ${response.status}`);
	}
	return response.json();
}

function getConfigPromise(): Promise<AuthConfig> {
	if (!configPromise) {
		configPromise = fetchAuthConfig();
	}
	return configPromise;
}

/**
 * Get the OIDC auth configuration (issuer, client_id, app_url).
 * Fetches from the backend on first call and caches the result.
 */
export async function getAuthConfig(): Promise<AuthConfig> {
	return getConfigPromise();
}

/**
 * Get the OIDC UserManager instance.
 * Lazily initialized from runtime auth config fetched from the backend.
 */
export async function getUserManager(): Promise<UserManager> {
	if (cachedManager) {
		return cachedManager;
	}
	const config = await getConfigPromise();
	cachedManager = new UserManager({
		authority: config.issuer,
		client_id: config.client_id,
		redirect_uri: `${config.app_url}/auth/callback`,
		post_logout_redirect_uri: `${config.app_url}/login`,
		response_type: 'code',
		scope: 'openid profile email urn:zitadel:iam:org:projects:roles',
		userStore: new WebStorageStateStore({ store: sessionStorage }),
		automaticSilentRenew: true,
	});
	return cachedManager;
}
