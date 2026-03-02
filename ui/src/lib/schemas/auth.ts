// Zod schemas for authentication-related forms
import { z } from 'zod';

export const inviteMemberSchema = z.object({
	email: z.string().email('Please enter a valid email address'),
	role: z.enum(['admin', 'member', 'viewer'], {
		message: 'Please select a valid role',
	}),
	firstName: z
		.string()
		.min(1, 'First name is required')
		.max(100, 'First name must be 100 characters or less'),
	lastName: z
		.string()
		.min(1, 'Last name is required')
		.max(100, 'Last name must be 100 characters or less'),
});

export type InviteMemberSchema = z.infer<typeof inviteMemberSchema>;
