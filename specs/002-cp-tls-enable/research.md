# Research: TLS Bring-Your-Own-Cert MVP

**Feature**: TLS termination for Flowplane control plane admin APIs
**Date**: 2025-09-27
**Input**: Technical Context from plan.md

## TLS Implementation Library

**Decision**: Use rustls with ring crypto backend
**Rationale**:
- Already present in Cargo.toml for existing mTLS functionality
- Memory-safe pure Rust implementation with no C dependencies
- Excellent performance and security track record
- Strong integration with tokio and axum ecosystems
- Consistent with existing xDS TLS implementation

**Alternatives considered**:
- native-tls: Platform-dependent, introduces C dependencies
- openssl: Security maintenance burden, FFI complexity

## Certificate Format and Validation

**Decision**: Support PEM format only with standard validation
**Rationale**:
- PEM is universal standard for TLS certificates
- Human-readable format for troubleshooting
- Supported by all major certificate authorities (Let's Encrypt, corporate PKI)
- Rust ecosystem has excellent PEM parsing libraries
- Matches existing xDS certificate handling

**Alternatives considered**:
- PKCS#12/P12: Binary format, more complex parsing
- DER: Binary format, less operator-friendly

## Configuration Approach

**Decision**: Environment variables with optional configuration file support
**Rationale**:
- Consistent with existing Flowplane configuration pattern
- Container-friendly (Docker, Kubernetes)
- Secure (no certificate content in config files, only paths)
- Follows 12-factor app principles
- Easy integration with deployment tooling

**Alternatives considered**:
- CLI flags only: Not persistent, harder for automation
- Configuration file only: Less flexible for containers

## Error Handling Strategy

**Decision**: Fail-fast at startup with structured error messages
**Rationale**:
- Prevents serving insecure HTTP when TLS intended
- Clear diagnostics for operational teams
- Consistent with constitutional "Validation Early" principle
- Better than runtime failures during certificate loading

**Alternatives considered**:
- Graceful degradation to HTTP: Security risk if unnoticed
- Runtime warnings: Could mask configuration issues

## Certificate Chain Handling

**Decision**: Support optional intermediate certificate file
**Rationale**:
- Required for many corporate PKI deployments
- Common pattern with Let's Encrypt certificates
- Separate file keeps certificate management clean
- Matches industry standard deployment practices

**Alternatives considered**:
- Concatenated chain in cert file: Less flexible, harder to manage
- Auto-discovery: Complex, unreliable in production

## Hot Reload vs Restart Requirement

**Decision**: Require service restart for certificate changes (MVP scope)
**Rationale**:
- Simpler implementation and testing
- Lower risk of runtime certificate loading failures
- Consistent with most production TLS deployments
- Can be enhanced in future iterations if needed

**Alternatives considered**:
- Hot reload: Complex implementation, higher failure risk
- File watching: Resource overhead, edge case handling

## Performance Optimization

**Decision**: Certificate validation at startup only, in-memory caching
**Rationale**:
- Minimal runtime overhead after startup
- Predictable performance characteristics
- Fails fast if certificates are invalid
- Suitable for long-running server processes

**Alternatives considered**:
- Per-request validation: Unnecessary overhead
- Lazy loading: Could hide configuration issues

## Integration with Existing Authentication

**Decision**: TLS termination at HTTP layer, transparent to authentication middleware
**Rationale**:
- Zero changes required to existing PAT authentication
- Clean separation of concerns (transport vs application security)
- Maintains existing audit logging and authorization flows
- Enables future authentication enhancements

**Alternatives considered**:
- TLS-aware authentication: Unnecessary complexity
- Certificate-based auth: Out of scope for MVP

## Documentation Approach

**Decision**: Comprehensive operational guides with working examples
**Rationale**:
- Reduces deployment friction for operators
- Covers common certificate sources (ACME, corporate PKI, self-signed)
- Includes troubleshooting for common issues
- Supports both container and traditional deployments

**Alternatives considered**:
- Minimal documentation: Higher support burden
- Implementation-focused docs: Less useful for operators