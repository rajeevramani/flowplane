/**
 * Shared constants for the Flowplane UI
 * Centralizes configuration values that are used across multiple components
 */

// HTTP Methods for route matching
export const HTTP_METHODS = [
	"GET",
	"POST",
	"PUT",
	"DELETE",
	"PATCH",
	"HEAD",
	"OPTIONS",
	"*",
] as const;

export type HttpMethod = (typeof HTTP_METHODS)[number];

// HTTP Redirect status codes
export const REDIRECT_CODES = [
	{ value: 301, label: "301 - Permanent" },
	{ value: 302, label: "302 - Found" },
	{ value: 303, label: "303 - See Other" },
	{ value: 307, label: "307 - Temporary" },
	{ value: 308, label: "308 - Permanent Redirect" },
] as const;

export type RedirectCode = (typeof REDIRECT_CODES)[number]["value"];

// Path match types for routing
export const PATH_MATCH_TYPES = [
	{ value: "prefix", label: "Prefix" },
	{ value: "exact", label: "Exact" },
	{ value: "regex", label: "Regex" },
	{ value: "template", label: "Template" },
] as const;

// Route action types
export const ROUTE_ACTION_TYPES = [
	{ value: "forward", label: "Forward" },
	{ value: "weighted", label: "Weighted" },
	{ value: "redirect", label: "Redirect" },
] as const;
