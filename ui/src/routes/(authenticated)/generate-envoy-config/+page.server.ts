import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async () => {
	// Redirect to dataplanes page - envoy config is now managed there
	redirect(301, '/dataplanes');
};
