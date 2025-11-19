import { redirect } from '@sveltejs/kit';
import type { LayoutServerLoad } from './$types';

export const load: LayoutServerLoad = async ({ cookies }) => {
	const sessionId = cookies.get('session_id');

	if (!sessionId) {
		throw redirect(302, '/login');
	}

	// Session validation is handled by the layout component
	// We just ensure the cookie exists here
	return {};
};
