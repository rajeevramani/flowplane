import { redirect } from '@sveltejs/kit';
import type { LayoutServerLoad } from './$types';

export const load: LayoutServerLoad = async ({ cookies }) => {
	const sessionToken = cookies.get('fp_session');

	if (!sessionToken) {
		throw redirect(302, '/login');
	}

	// Session validation is handled by the layout component
	// We just ensure the cookie exists here
	return {};
};
