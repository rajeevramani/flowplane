import { z } from 'zod';

export const CertificateMetadataSchema = z.object({
	id: z.string(),
	proxyId: z.string(),
	spiffeUri: z.string(),
	serialNumber: z.string(),
	issuedAt: z.string(),
	expiresAt: z.string(),
	isValid: z.boolean(),
	isExpired: z.boolean(),
	isRevoked: z.boolean(),
	revokedAt: z.string().nullable(),
	revokedReason: z.string().nullable()
});

export const ListCertificatesResponseSchema = z.object({
	certificates: z.array(CertificateMetadataSchema),
	total: z.number(),
	limit: z.number(),
	offset: z.number()
});

export type CertificateMetadataData = z.infer<typeof CertificateMetadataSchema>;
export type ListCertificatesResponseData = z.infer<typeof ListCertificatesResponseSchema>;
