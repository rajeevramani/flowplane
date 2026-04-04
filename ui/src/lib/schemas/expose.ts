import { z } from 'zod';

/**
 * Zod schema for the expose form.
 * Matches backend validation in src/api/handlers/expose.rs and src/validation/mod.rs.
 */
export const ExposeFormSchema = z.object({
	name: z
		.string()
		.min(1, 'Service name is required')
		.max(100, 'Name must be 100 characters or fewer')
		.regex(
			/^[a-zA-Z0-9][a-zA-Z0-9._-]*$/,
			'Name must start with alphanumeric and contain only letters, digits, dots, hyphens, or underscores'
		),
	upstream: z
		.string()
		.min(1, 'Upstream URL is required')
		.trim()
		.refine((val) => !/\s/.test(val), { message: 'Upstream must not contain spaces' })
		.refine(
			(val) => {
				// Strip scheme if present
				const withoutScheme = val.replace(/^https?:\/\//, '');
				// Strip path
				const hostPort = withoutScheme.split('/')[0];
				// Must have host:port
				const parts = hostPort.split(':');
				if (parts.length < 2) return false;
				const port = parseInt(parts[parts.length - 1], 10);
				return !isNaN(port) && port > 0 && port <= 65535;
			},
			{ message: 'Upstream must be in host:port or http://host:port format' }
		),
	port: z
		.number()
		.int()
		.min(10001, 'Port must be between 10001 and 10020')
		.max(10020, 'Port must be between 10001 and 10020')
		.optional(),
	paths: z
		.array(
			z.string().refine((val) => val.startsWith('/'), {
				message: 'Each path must start with /'
			})
		)
		.min(1, 'At least one path is required')
		.optional()
});

export type ExposeFormData = z.infer<typeof ExposeFormSchema>;

export const ExposeResponseSchema = z.object({
	name: z.string(),
	upstream: z.string(),
	port: z.number(),
	paths: z.array(z.string()),
	cluster: z.string(),
	route_config: z.string(),
	listener: z.string()
});

export type ExposeResponseData = z.infer<typeof ExposeResponseSchema>;
