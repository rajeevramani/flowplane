// Zod schemas for authentication forms
import { z } from 'zod';

export const loginSchema = z.object({
	email: z.string().email('Please enter a valid email address'),
	password: z.string().min(1, 'Password is required'),
});

export type LoginSchema = z.infer<typeof loginSchema>;

export const bootstrapSchema = z
	.object({
		name: z.string().min(1, 'Name is required'),
		email: z.string().email('Please enter a valid email address'),
		password: z.string().min(8, 'Password must be at least 8 characters'),
		confirmPassword: z.string().min(1, 'Please confirm your password'),
	})
	.refine((data) => data.password === data.confirmPassword, {
		message: "Passwords don't match",
		path: ['confirmPassword'],
	});

export type BootstrapSchema = z.infer<typeof bootstrapSchema>;

export const registerSchema = z
	.object({
		name: z.string().min(1, 'Name is required'),
		password: z.string().min(8, 'Password must be at least 8 characters'),
		confirmPassword: z.string().min(1, 'Please confirm your password'),
	})
	.refine((data) => data.password === data.confirmPassword, {
		message: "Passwords don't match",
		path: ['confirmPassword'],
	});

export type RegisterSchema = z.infer<typeof registerSchema>;

export const createInvitationSchema = z.object({
	email: z.string().email('Please enter a valid email address'),
	role: z.enum(['admin', 'member', 'viewer'], {
		message: 'Please select a valid role',
	}),
});

export type CreateInvitationSchema = z.infer<typeof createInvitationSchema>;
