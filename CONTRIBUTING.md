# Contributing to Flowplane

Thank you for your interest in contributing to Flowplane. This guide covers development setup, code standards, and the contribution workflow.

## Prerequisites

- **Rust** 1.89+ (edition 2021)
- **Node.js** 20+ with pnpm
- **PostgreSQL** 15+
- **protoc** (Protocol Buffers compiler)
- **Docker/Podman** (optional, for containerized development)

## Development Setup

### Backend (Rust)

```bash
# Clone the repository
git clone https://github.com/rajeevramani/flowplane.git
cd flowplane

# Build the backend
cargo build

# Run the control plane
cargo run

# The API is available at http://localhost:8080
# The xDS server is available at localhost:18000
```

### Frontend (SvelteKit)

```bash
cd ui

# Install dependencies
pnpm install

# Start development server
pnpm dev

# The UI is available at http://localhost:3000
```

### Full Stack with Docker Compose

```bash
# Start backend + UI
make up

# Start with tracing (Jaeger)
make up-tracing

# Start with mTLS (Vault)
make up-mtls

# Add httpbin test service
make up HTTPBIN=1

# Stop all services
make down

# View all options
make help
```

### Database

The development database requires a running PostgreSQL instance. Set the connection URL via `DATABASE_URL=postgres://flowplane:flowplane@localhost:5432/flowplane`. Logs are written to `data/logs/flowplane-*.log`.

## Code Standards

### Rust (Backend)

- **Error Handling**: Never use `unwrap()` or `expect()` in production code. Always use `Result` types with proper error propagation.
- **Async**: No blocking operations in async code. Use `tokio::sleep`, not `std::thread::sleep`.
- **Ownership**: Prefer `&str` over `String` for function parameters. Use `Arc` for shared async ownership.
- **Domain Types**: Use the newtype pattern for domain identifiers (`UserId(i64)`, `TeamId(i64)`).
- **Testing**: All code must have comprehensive tests. Untested code will not be accepted.

### Frontend (SvelteKit/TypeScript)

- **Type Safety**: Never use the `any` type. Use proper TypeScript types or `unknown`.
- **Components**: Build reusable UI components. Extract common patterns into shared components.
- **Validation**: Use Zod schemas for all form validation and API response parsing.
- **Styling**: Use Tailwind design tokens (`text-blue-600`), not arbitrary values (`text-[#1234AB]`).
- **Conventions**: Follow SvelteKit file conventions: `+page.svelte`, `+page.server.ts`, `+layout.svelte`.
- **Props**: Component props must have explicit types using `ComponentProps<typeof Component>`.

### Database

- **Queries**: Never write N+1 queries. Use JOINs for related data.
- **Transactions**: Use transactions for multi-step operations.
- **Type Safety**: Use SQLx compile-time checked `query_as!` macros.

### Architecture

- **DRY Principle**: Search the codebase before writing new code. Reuse existing implementations.
- **Configuration**: Externalize all config. Priority: Environment variables > Config file > CLI args > Defaults.
- **Multi-tenancy**: Always enforce team isolation in queries and handlers.
- **API Consistency**: The three API paradigms (Native API, Gateway API, Platform API) must behave consistently.

## Development Workflow

### Branch Strategy

We use release branches: `feature/task-X → release/v0.0.X → main`

1. **Start a task**: Review the feature in `.local/features/features.md`
2. **Create feature branch** from release (not main):
   ```bash
   git checkout release/v0.0.X
   git checkout -b feature/task-description
   ```
3. **All subtasks** use the same feature branch
4. **After task completion**: Merge feature branch into release branch
5. **After all release tasks**: Create PR to merge release into main, then tag the release

### Commit Guidelines

- Write minimal, descriptive commit messages
- No emojis in commit messages
- No author attribution in messages

### Pre-Commit Checklist

Before committing, verify:

- [ ] No `unwrap()` or `expect()` in production code
- [ ] Comprehensive tests written and passing
- [ ] No N+1 queries, database operations optimized
- [ ] No code duplication (DRY principle followed)
- [ ] `cargo fmt` passes with no changes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test` passes with 0 failures

Run all checks:

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test
```

For frontend:

```bash
cd ui && pnpm check
```

### Pull Request Process

1. Ensure all pre-commit checks pass
2. Keep PRs focused and small
3. Reference the related feature from `.local/features/features.md`
4. Wait for CI to pass before requesting review
5. Address review feedback promptly

## Testing

### Test Pyramid

- **Many unit tests**: Test individual functions and modules
- **Some integration tests**: Test component interactions
- **Few E2E tests**: Test complete user flows

### What to Test

- Unit logic and business rules
- API handlers and endpoints
- Cross-paradigm consistency (verify identical xDS output)
- Team isolation and authorization

### Running Tests

```bash
# Run all backend tests
cargo test

# Run specific test module
cargo test auth::

# Run with output
cargo test -- --nocapture

# Run PostgreSQL integration tests (uses testcontainers — Docker/Podman required)
cargo test --features postgres_tests

# Run E2E tests (requires control plane not running on same ports)
RUN_E2E=1 cargo test -p flowplane --test e2e -- --ignored --test-threads=1

# Frontend type checking
cd ui && pnpm check
```

## Reporting Issues

### Bug Reports

Include:
- Steps to reproduce
- Expected behavior
- Actual behavior
- Environment (OS, Rust version, Node version)
- Relevant logs

### Feature Requests

Include:
- Use case description
- Proposed solution
- Alternatives considered

### Security Issues

For security vulnerabilities, please email the maintainers directly instead of opening a public issue.

## Project Structure

```
flowplane/
├── src/                    # Rust backend source
│   ├── api/               # REST API handlers
│   ├── auth/              # Authentication and authorization
│   ├── domain/            # Domain types and business logic
│   ├── services/          # Business services
│   ├── storage/           # Database repositories
│   ├── mcp/               # MCP protocol tools
│   ├── internal_api/      # Internal operations layer
│   └── xds/               # xDS protocol implementation
├── ui/                     # SvelteKit frontend
│   ├── src/lib/           # Shared components and utilities
│   └── src/routes/        # Page routes
├── agents/                 # AI agent implementations
├── migrations/             # PostgreSQL migrations
├── tests/                  # Integration and E2E tests
├── docs/                   # Documentation
└── .local/features/        # Feature tracking (internal)
```

## License

By contributing to Flowplane, you agree that your contributions will be licensed under the MIT License.
