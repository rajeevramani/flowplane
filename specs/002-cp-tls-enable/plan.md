
# Implementation Plan: TLS Bring-Your-Own-Cert MVP

**Branch**: `002-cp-tls-enable` | **Date**: 2025-09-27 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/Users/rajeevramani/workspace/projects/flowplane/specs/002-cp-tls-enable/spec.md`

## Execution Flow (/plan command scope)
```
1. Load feature spec from Input path
   → If not found: ERROR "No feature spec at {path}"
2. Fill Technical Context (scan for NEEDS CLARIFICATION)
   → Detect Project Type from file system structure or context (web=frontend+backend, mobile=app+api)
   → Set Structure Decision based on project type
3. Fill the Constitution Check section based on the content of the constitution document.
4. Evaluate Constitution Check section below
   → If violations exist: Document in Complexity Tracking
   → If no justification possible: ERROR "Simplify approach first"
   → Update Progress Tracking: Initial Constitution Check
5. Execute Phase 0 → research.md
   → If NEEDS CLARIFICATION remain: ERROR "Resolve unknowns"
6. Execute Phase 1 → contracts, data-model.md, quickstart.md, agent-specific template file (e.g., `CLAUDE.md` for Claude Code, `.github/copilot-instructions.md` for GitHub Copilot, `GEMINI.md` for Gemini CLI, `QWEN.md` for Qwen Code or `AGENTS.md` for opencode).
7. Re-evaluate Constitution Check section
   → If new violations: Refactor design, return to Phase 1
   → Update Progress Tracking: Post-Design Constitution Check
8. Plan Phase 2 → Describe task generation approach (DO NOT create tasks.md)
9. STOP - Ready for /tasks command
```

**IMPORTANT**: The /plan command STOPS at step 7. Phases 2-4 are executed by other commands:
- Phase 2: /tasks command creates tasks.md
- Phase 3-4: Implementation execution (manual or via tools)

## Summary
Implement TLS termination for Flowplane control plane admin APIs using externally-provided certificates (bring-your-own-cert model). Features include environment variable configuration toggle, PEM certificate/key validation at startup, fail-fast error handling, backward compatibility (HTTP default), and comprehensive documentation for ACME, corporate PKI, and self-signed certificate workflows. Maintains existing mTLS for xDS communication unchanged and preserves personal access token authentication behavior.

## Technical Context
**Language/Version**: Rust 1.75+ (edition 2021)
**Primary Dependencies**: Axum (HTTP server), tonic (gRPC/TLS), rustls (TLS implementation), tokio (async runtime), clap (CLI), serde (serialization), thiserror (error handling)
**Storage**: Configuration files (PEM certificates), existing SQLite/PostgreSQL for audit logging
**Testing**: cargo test, axum-test (integration), tokio-test (async), proptest (property-based)
**Target Platform**: Linux server, macOS development
**Project Type**: single - Rust control plane application with TLS extension
**Performance Goals**: <50ms TLS handshake, <10ms certificate validation, existing API performance unchanged
**Constraints**: Zero breaking changes, backward compatibility required, no hot certificate reload (restart required)
**Scale/Scope**: Enterprise TLS termination, support multiple certificate authorities, comprehensive operational documentation

## Constitution Check
*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

**✅ I. Structured Configs First**: TLS configuration will use structured models (TlsConfig, CertificateBundle, ValidationError) with strong typing and validation before server startup.

**✅ II. Validation Early**: All certificate file validation, PEM parsing, and key/cert matching will happen at startup with descriptive errors before any network binding.

**✅ III. Test-First Development**: TLS configuration, certificate validation, and HTTPS server setup will be developed using TDD with comprehensive test coverage (>90%).

**✅ IV. Idempotent Resource Building**: TLS configuration loading and server setup will be atomic and safe to retry. Configuration state will be derivable from files without side effects.

**✅ V. Security by Default**: TLS implementation uses rustls with secure defaults, certificate validation is mandatory, no private key exposure in logs, existing xDS mTLS preserved.

**✅ VI. Application Stability**: TLS termination is purely additive - existing HTTP mode continues working unchanged. New TLS configuration is optional and backward compatible.

**✅ VII. DRY Principle & Rust Excellence**: Common TLS logic extracted into reusable modules, proper error types with thiserror, leverage Rust's type system for compile-time certificate validation guarantees.

## Project Structure

### Documentation (this feature)
```
specs/002-cp-tls-enable/
├── plan.md              # This file (/plan command output)
├── research.md          # Phase 0 output (/plan command)
├── data-model.md        # Phase 1 output (/plan command)
├── quickstart.md        # Phase 1 output (/plan command)
├── contracts/           # Phase 1 output (/plan command)
│   ├── tls-config.yaml  # TLS configuration contract
│   └── server-tls.yaml  # HTTPS server contract
└── tasks.md             # Phase 2 output (/tasks command - NOT created by /plan)
```

### Source Code (repository root)
```
src/
├── config/                  # Configuration models (EXTEND)
│   ├── tls.rs              # TLS configuration structures (NEW)
│   └── mod.rs              # Export TLS config (EXTEND)
├── api/                     # REST API (EXTEND)
│   ├── server.rs           # TLS-enabled server setup (EXTEND)
│   └── mod.rs              # Module exports (EXTEND)
├── cli/                     # Command line interface (EXTEND)
│   ├── tls.rs              # TLS configuration commands (NEW)
│   └── mod.rs              # CLI exports (EXTEND)
├── utils/                   # Utilities (EXTEND)
│   ├── certificates.rs     # Certificate validation utilities (NEW)
│   └── mod.rs              # Utility exports (EXTEND)
├── errors/                  # Error handling (EXTEND)
│   ├── tls.rs              # TLS-specific errors (NEW)
│   └── mod.rs              # Error exports (EXTEND)
└── main.rs                  # Application entry point (EXTEND)

tests/
├── tls/                     # TLS-specific tests (NEW)
│   ├── contract/           # API contract tests
│   ├── integration/        # End-to-end TLS flows
│   └── unit/               # Certificate validation, config tests
├── contract/                # Existing contract tests (NO CHANGES)
├── integration/             # Existing integration tests (EXTEND)
└── unit/                    # Existing unit tests (NO CHANGES)

docs/                        # Documentation (EXTEND)
├── tls-setup.md            # Bring-your-own-cert guide (NEW)
├── deployment-examples/     # Reference configurations (NEW)
│   ├── docker-compose.yml  # Docker example with TLS
│   └── systemd.service     # Systemd service template
└── troubleshooting.md      # TLS troubleshooting guide (NEW)
```

**Structure Decision**: Single project architecture extending existing Flowplane structure. TLS functionality is added as configuration extensions in `src/config/tls.rs`, server modifications in `src/api/server.rs`, and comprehensive test coverage in `tests/tls/`. Existing modules remain unchanged to preserve application stability. Documentation includes operational guides and deployment examples.

## Phase 0: Outline & Research
1. **Extract unknowns from Technical Context** above:
   - For each NEEDS CLARIFICATION → research task
   - For each dependency → best practices task
   - For each integration → patterns task

2. **Generate and dispatch research agents**:
   ```
   For each unknown in Technical Context:
     Task: "Research {unknown} for {feature context}"
   For each technology choice:
     Task: "Find best practices for {tech} in {domain}"
   ```

3. **Consolidate findings** in `research.md` using format:
   - Decision: [what was chosen]
   - Rationale: [why chosen]
   - Alternatives considered: [what else evaluated]

**Output**: research.md with all NEEDS CLARIFICATION resolved

## Phase 1: Design & Contracts
*Prerequisites: research.md complete*

1. **Extract entities from feature spec** → `data-model.md`:
   - Entity name, fields, relationships
   - Validation rules from requirements
   - State transitions if applicable

2. **Generate API contracts** from functional requirements:
   - For each user action → endpoint
   - Use standard REST/GraphQL patterns
   - Output OpenAPI/GraphQL schema to `/contracts/`

3. **Generate contract tests** from contracts:
   - One test file per endpoint
   - Assert request/response schemas
   - Tests must fail (no implementation yet)

4. **Extract test scenarios** from user stories:
   - Each story → integration test scenario
   - Quickstart test = story validation steps

5. **Update agent file incrementally** (O(1) operation):
   - Run `.specify/scripts/bash/update-agent-context.sh claude`
     **IMPORTANT**: Execute it exactly as specified above. Do not add or remove any arguments.
   - If exists: Add only NEW tech from current plan
   - Preserve manual additions between markers
   - Update recent changes (keep last 3)
   - Keep under 150 lines for token efficiency
   - Output to repository root

**Output**: data-model.md, /contracts/*, failing tests, quickstart.md, agent-specific file

## Phase 2: Task Planning Approach
*This section describes what the /tasks command will do - DO NOT execute during /plan*

**Task Generation Strategy**:
- Configuration model tasks: TlsConfig, CertificateBundle, TlsError types
- Certificate validation tasks: PEM parsing, certificate/key matching, file access
- Server integration tasks: Axum TLS setup, rustls configuration, startup logic
- Contract test tasks for TLS configuration and HTTPS server behavior [P]
- Integration test tasks for end-to-end TLS scenarios
- Documentation tasks: operational guides, deployment examples
- CLI enhancement tasks: TLS-related commands and configuration validation

**Ordering Strategy**:
- **Phase 1**: Configuration models and error types (foundation layer)
- **Phase 2**: Contract tests [P] → Certificate validation logic [P] (TDD parallel)
- **Phase 3**: Server integration → CLI enhancements → Documentation (dependency order)
- **Phase 4**: Integration tests → Performance validation → Security review

**Specific Task Categories**:
1. **Configuration Tasks**: TLS config models, environment variable parsing, validation rules
2. **Test Tasks [P]**: Contract tests for TLS config and server behavior, unit tests for certificate validation
3. **Certificate Tasks [P]**: PEM parsing, certificate/key validation, expiration checking
4. **Server Tasks**: Axum TLS integration, rustls setup, startup failure handling
5. **CLI Tasks**: TLS configuration commands, certificate validation tools
6. **Documentation Tasks**: Operational guides, deployment examples, troubleshooting guides
7. **Integration Tasks**: End-to-end TLS flows, authentication preservation, performance validation

**Estimated Output**: 32-38 numbered, ordered tasks with 16-20 marked [P] for parallel execution

**Code Integration Strategy**:
- Extend existing `src/config/` module with TLS configuration support
- Integrate TLS into existing `src/api/server.rs` startup logic
- Add certificate utilities to `src/utils/` for reusability
- Enhance existing CLI in `src/cli/` with TLS commands
- Preserve all existing functionality (HTTP mode default, xDS mTLS unchanged)

**IMPORTANT**: This phase is executed by the /tasks command, NOT by /plan

## Phase 3+: Future Implementation
*These phases are beyond the scope of the /plan command*

**Phase 3**: Task execution (/tasks command creates tasks.md)  
**Phase 4**: Implementation (execute tasks.md following constitutional principles)  
**Phase 5**: Validation (run tests, execute quickstart.md, performance validation)

## Complexity Tracking
*Fill ONLY if Constitution Check has violations that must be justified*

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| [e.g., 4th project] | [current need] | [why 3 projects insufficient] |
| [e.g., Repository pattern] | [specific problem] | [why direct DB access insufficient] |


## Progress Tracking
*This checklist is updated during execution flow*

**Phase Status**:
- [x] Phase 0: Research complete (/plan command)
- [x] Phase 1: Design complete (/plan command)
- [x] Phase 2: Task planning complete (/plan command - describe approach only)
- [ ] Phase 3: Tasks generated (/tasks command)
- [ ] Phase 4: Implementation complete
- [ ] Phase 5: Validation passed

**Gate Status**:
- [x] Initial Constitution Check: PASS
- [x] Post-Design Constitution Check: PASS
- [x] All NEEDS CLARIFICATION resolved
- [x] Complexity deviations documented (None required)

---
*Based on Constitution v1.1.0 - See `.specify/memory/constitution.md`*
