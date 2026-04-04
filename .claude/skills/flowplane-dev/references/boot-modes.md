# Boot Modes Reference

## Dev Mode — Via CLI

### Prerequisites
- Docker or Podman running
- Flowplane CLI built (`cargo build --bin flowplane-cli`)

### Steps
```bash
# One command does everything
flowplane init --with-envoy --with-httpbin
```

### What happens
1. Detects container runtime (Docker/Podman) via socket probing
2. Generates a 43-char base64 dev token (or reuses existing from `~/.flowplane/credentials`)
3. Writes `docker-compose-dev.yml` to `~/.flowplane/`
4. Writes Envoy bootstrap config to `~/.flowplane/envoy/` (if `--with-envoy`)
5. Removes stale `flowplane-network` (avoids Compose label conflict)
6. Starts containers: PostgreSQL + control plane + optional Envoy/httpbin
7. Waits up to 60s for control plane health (`/health` endpoint)
8. Writes token to `~/.flowplane/credentials` (plain text)
9. Updates `~/.flowplane/config.toml` with `base_url`, `team=default`, `org=dev-org`

### Services after boot
| Service | URL | Notes |
|---|---|---|
| REST API | `http://localhost:8080/api/v1/` | |
| Swagger UI | `http://localhost:8080/swagger-ui/` | |
| xDS gRPC | `localhost:18000` | |
| Envoy | `http://localhost:10000` | Only with `--with-envoy` |
| Envoy Admin | `http://localhost:9901` | Only with `--with-envoy` |
| httpbin | `http://localhost:8000` | Only with `--with-httpbin` |

### Verify
```bash
# Check API responds
curl http://localhost:8080/api/v1/auth/mode
# Expected: {"auth_mode":"dev"}

# Check CLI works
flowplane status
flowplane auth whoami
# Expected: dev@flowplane.local, org: dev-org, team: default
```

### Auto-seeded resources (on startup)
- Organization: `dev-org` (id: `dev-org-id`)
- Team: `default` (id: `dev-default-team-id`)
- User: `dev@flowplane.local` (id: `dev-user-id`)
- Dataplane: `dev-dataplane` (id: `dev-dataplane-id`)
- Org membership: dev user as org admin
- Team membership: dev user in default team

All inserts are idempotent (`ON CONFLICT DO NOTHING`). Safe on every restart.

### Stop
```bash
flowplane down           # Stop containers
flowplane down --volumes # Stop and remove data
```

---

## Dev Mode — Via Make

Not recommended — `flowplane init` is simpler. But possible:

```bash
# Set token manually
export FLOWPLANE_DEV_TOKEN=$(openssl rand -base64 32)
docker compose -f docker-compose-dev.yml up -d
```

---

## Prod Mode — Via Make

### Prerequisites
- Docker or Podman running
- No previous dev-mode network conflict (see gotcha below)

### Steps
```bash
# Step 1: Start the stack
make up ENVOY=1 HTTPBIN=1

# First run auto-detects missing .env.zitadel and runs setup-zitadel.sh
# This creates: Zitadel project, SPA app, machine user, service account
# Outputs are saved to .env.zitadel (auto-sourced by docker-compose.yml)

# Step 2: Seed demo data
make seed

# Step 3: Check credentials
make seed-info
```

### Services after boot
| Service | URL | Notes |
|---|---|---|
| REST API | `http://localhost:8080/api/v1/` | |
| Swagger UI | `http://localhost:8080/swagger-ui/` | |
| UI | `http://localhost:8080/` | SvelteKit app |
| Zitadel Console | `http://localhost:8081` | Admin: `zitadel-admin@zitadel.localhost` / `Password1!` |
| xDS gRPC | `localhost:50051` | |
| Envoy | `http://localhost:10000` | Only with `ENVOY=1` |
| Envoy Admin | `http://localhost:9901` | Only with `ENVOY=1` |
| httpbin | `http://localhost:8000` | Only with `HTTPBIN=1` |
| MockBank | `http://localhost:3001/v2/api/customers` | Only with `MOCKBACKEND=1` |

### Default credentials
| User | Password | Role |
|---|---|---|
| `admin@flowplane.local` | `Flowplane1!` | Platform admin |
| Demo user | Created by `make seed` | Org admin |

### Verify
```bash
# Check auth mode
curl http://localhost:8080/api/v1/auth/mode
# Expected: {"auth_mode":"prod","oidc_issuer":"http://localhost:8081",...}

# Login via CLI
flowplane auth login
# Opens browser for PKCE flow

# Check identity
flowplane auth whoami
```

### Make targets
| Target | Purpose |
|---|---|
| `make up` | Start backend + UI + Zitadel |
| `make up ENVOY=1` | + Envoy proxy |
| `make up HTTPBIN=1` | + httpbin demo backend |
| `make up MOCKBACKEND=1` | + MockBank API |
| `make up-mtls` | + Vault for mTLS certificates |
| `make up-tracing` | + Jaeger for distributed tracing |
| `make up-full` | All of the above |
| `make down` | Stop all services |
| `make clean` | Remove volumes and orphans |
| `make logs` | Tail all service logs |
| `make status` | Show running containers |
| `make setup-zitadel` | (Re)configure Zitadel |
| `make seed` | Seed demo data |
| `make seed-info` | Show credentials |

### Stop
```bash
make down       # Stop containers
make clean      # Stop + remove volumes
```

---

## Docker Services

### Prod mode (`docker-compose.yml`)
| Service | Image | Purpose |
|---|---|---|
| `control-plane` | `flowplane:latest` | Rust backend (REST + MCP + xDS) |
| `flowplane-ui` | `flowplane-ui:latest` | SvelteKit frontend |
| `flowplane-pg` | `postgres:17-alpine` | PostgreSQL database |
| `zitadel` | `ghcr.io/zitadel/zitadel` | OIDC identity provider |
| `zitadel-db` | `postgres:17-alpine` | Zitadel's own PostgreSQL |

### Dev mode (`docker-compose-dev.yml`)
| Service | Image | Purpose |
|---|---|---|
| `control-plane` | `flowplane:latest` | Backend with `FLOWPLANE_AUTH_MODE=dev` |
| `flowplane-pg` | `postgres:17-alpine` | PostgreSQL |
| `envoy` (profile) | `envoyproxy/envoy:v1.31-latest` | Optional data plane |
| `httpbin` (profile) | `kennethreitz/httpbin` | Optional demo backend |

### Sidecar compose files
| File | Adds |
|---|---|
| `docker-compose-envoy.yml` | Envoy proxy (ports 10000-10020, admin 9901) |
| `docker-compose-httpbin.yml` | httpbin (port 8000) |
| `docker-compose-mockbackend.yml` | MockBank API (port 3001) |

---

## Common Boot Failures

### Network conflict when switching modes
**Error:** `network flowplane-network: ... label "com.docker.compose.network" is empty`

**Cause:** `flowplane init` creates `flowplane-network` via `docker network create` (outside Compose), but `make up` expects Compose to own it.

**Fix:**
```bash
docker network rm flowplane-network
make up ENVOY=1 HTTPBIN=1
```

### Port already in use
**Error:** `Bind for 0.0.0.0:8080 failed: port is already allocated`

**Cause:** Previous stack not stopped, or another service on port 8080.

**Fix:**
```bash
make down     # or: flowplane down
# Then retry
```

### Zitadel not ready
**Error:** `setup-zitadel.sh` fails with connection refused

**Cause:** Zitadel takes 30-60s to initialize on first run.

**Fix:** Wait and retry:
```bash
make setup-zitadel
```

### Build fails (image not found)
**Error:** `image flowplane:latest not found`

**Cause:** Docker images not built yet.

**Fix:**
```bash
make build           # Build all images
# or
make build-backend   # Backend only
make build-ui        # Frontend only
```
