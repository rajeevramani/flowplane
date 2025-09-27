# Feature Specification: Control Plane API Authentication System

**Feature Branch**: `001-control-plane-auth`
**Created**: 2025-09-26
**Status**: Draft
**Input**: User description: "We need to add first-class API authentication to Flowplane using personal access tokens, with a path to JWT/OIDC later. The feature should let control-plane admins issue long-lived opaque tokens (one-shot reveal, hashed at rest) that carry a set of scopes like clusters:read, listeners:write, etc.; requests to our Axum API must carry a bearer token, and middleware should resolve it, check scope/role against the endpoint's requirements, and return 401/403 when missing. Token lifecycle—create, rotate, revoke—must be auditable: write change and usage records into the existing audit_log with correlation IDs, and refuse operations once a token is deactivated or expired. Use proven crates for hashing and JWT/OIDC verification so we can extend the same authorization layer to accept federated JWTs later (validate issuer/audience/signature, map claims to scopes). Document operator workflows (issuing tokens, rotating them, wiring CI/CD). This is for the CP Admin API only and not for any of the generated data-plane traffic or the upstream route configs."

## User Scenarios & Testing

### Primary User Story
As a control-plane administrator, I want to securely authenticate API requests to the Flowplane control plane using personal access tokens with granular scope-based permissions, so that I can control who has access to specific operations (clusters, routes, listeners) and maintain an audit trail of all authentication events and API usage.

### Acceptance Scenarios
1. **Given** I am a control-plane admin, **When** I create a new personal access token with specific scopes (e.g., "clusters:read,listeners:write"), **Then** the system generates a secure opaque token, shows it once, stores only the hashed version, and logs the creation event
2. **Given** a valid bearer token with appropriate scopes, **When** I make an API request to a protected endpoint, **Then** the system validates the token, checks scopes against endpoint requirements, and allows access with audit logging
3. **Given** an expired or revoked token, **When** I attempt to use it for API access, **Then** the system returns 401 Unauthorized and logs the failed attempt with correlation ID
4. **Given** a valid token with insufficient scopes, **When** I try to access a restricted endpoint, **Then** the system returns 403 Forbidden and logs the authorization failure
5. **Given** existing tokens in the system, **When** I rotate or revoke a token, **Then** the old token immediately becomes invalid and the change is auditably logged

### Edge Cases
- What happens when a token is used after its expiration date?
- How does the system handle malformed bearer tokens in requests?
- What occurs when an admin tries to create tokens with invalid scope combinations?
- How are concurrent token operations (create/revoke) handled safely?
- What safeguards exist to prevent token enumeration attacks?

## Requirements

### Functional Requirements

- **FR-001**: System MUST allow control-plane admins to create personal access tokens with configurable expiration dates and specific scope permissions
- **FR-002**: System MUST generate cryptographically secure opaque tokens that are revealed only once during creation and stored as secure hashes
- **FR-003**: System MUST support granular scopes for API operations including clusters:read, clusters:write, routes:read, routes:write, listeners:read, listeners:write
- **FR-004**: System MUST authenticate API requests by validating bearer tokens in Authorization headers and matching scopes to endpoint requirements
- **FR-005**: System MUST return appropriate HTTP status codes (401 for invalid tokens, 403 for insufficient scopes) with descriptive error messages
- **FR-006**: System MUST provide token lifecycle management operations including creation, rotation, revocation, and listing of active tokens
- **FR-007**: System MUST log all authentication events, authorization decisions, and token lifecycle changes to the existing audit log with correlation IDs
- **FR-008**: System MUST immediately invalidate expired, revoked, or rotated tokens and refuse all operations using them
- **FR-009**: System MUST support future extension to JWT/OIDC federation by implementing a pluggable authentication layer that can validate external tokens
- **FR-010**: System MUST provide secure token storage with protection against timing attacks and token enumeration
- **FR-011**: System MUST validate token scopes against specific endpoint requirements before allowing access to protected resources
- **FR-012**: System MUST generate unique correlation IDs for each request to enable tracing authentication and authorization decisions through audit logs
- **FR-013**: System MUST support token metadata including creation date, last used date, creator identity, and human-readable descriptions
- **FR-014**: System MUST prevent privilege escalation by ensuring tokens cannot be used to create tokens with higher privileges than the creator
- **FR-015**: System MUST provide administrative endpoints for token management that are themselves properly authenticated and authorized

### Key Entities

- **Personal Access Token**: Represents an authentication credential with unique identifier, secure hash, expiration date, granted scopes, creation metadata, and current status (active/revoked/expired)
- **Token Scope**: Defines granular permissions for API operations, organized by resource type and access level (read/write), with hierarchical relationships
- **Authentication Session**: Represents a validated request context including resolved token identity, granted permissions, correlation ID, and audit trail information
- **Audit Log Entry**: Records authentication and authorization events with timestamps, token identifiers, requested resources, decisions made, and correlation IDs for traceability
- **Admin Identity**: Represents the control-plane administrator who creates and manages tokens, with their own permissions and ability to delegate specific scopes

## Review & Acceptance Checklist

### Content Quality
- [x] No implementation details (languages, frameworks, APIs)
- [x] Focused on user value and business needs
- [x] Written for non-technical stakeholders
- [x] All mandatory sections completed

### Requirement Completeness
- [x] No [NEEDS CLARIFICATION] markers remain
- [x] Requirements are testable and unambiguous
- [x] Success criteria are measurable
- [x] Scope is clearly bounded
- [x] Dependencies and assumptions identified

## Execution Status

- [x] User description parsed
- [x] Key concepts extracted
- [x] Ambiguities marked
- [x] User scenarios defined
- [x] Requirements generated
- [x] Entities identified
- [x] Review checklist passed