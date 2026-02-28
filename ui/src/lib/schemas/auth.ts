// Zod schemas for authentication-related forms
import { z } from 'zod';

export const createInvitationSchema = z.object({
	email: z.string().email('Please enter a valid email address'),
	role: z.enum(['admin', 'member', 'viewer'], {
		message: 'Please select a valid role',
	}),
});

export type CreateInvitationSchema = z.infer<typeof createInvitationSchema>;
