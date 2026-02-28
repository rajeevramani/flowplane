import { UserManager, WebStorageStateStore } from 'oidc-client-ts';

const ZITADEL_ISSUER = import.meta.env.VITE_ZITADEL_ISSUER || 'http://localhost:8081';
const ZITADEL_CLIENT_ID = import.meta.env.VITE_ZITADEL_CLIENT_ID || '';
const APP_URL = import.meta.env.VITE_APP_URL || 'http://localhost:5173';

export const userManager = new UserManager({
	authority: ZITADEL_ISSUER,
	client_id: ZITADEL_CLIENT_ID,
	redirect_uri: `${APP_URL}/auth/callback`,
	post_logout_redirect_uri: `${APP_URL}/login`,
	response_type: 'code',
	scope: 'openid profile email urn:zitadel:iam:org:projects:roles',
	userStore: new WebStorageStateStore({ store: sessionStorage }),
	automaticSilentRenew: true,
});
