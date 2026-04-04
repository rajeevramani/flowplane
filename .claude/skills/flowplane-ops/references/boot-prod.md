# Prod Mode Boot Recipe

## Prerequisites
- Docker or Podman running
- Flowplane images built (`make build`)

## Boot (3 steps)
```bash
# Step 1: Start the stack
make up ENVOY=1 HTTPBIN=1

# First run auto-detects missing .env.zitadel and runs setup-zitadel.sh
# Wait for all services to become healthy (~30-60s for Zitadel)

# Step 2: Seed demo data
make seed

# Step 3: Check credentials
make seed-info
```

## What Happens
1. `make up` starts: PostgreSQL, Zitadel + its DB, control plane, UI
2. On first run, auto-runs `scripts/setup-zitadel.sh` (creates project, SPA app, machine user)
3. Outputs saved to `.env.zitadel` (auto-sourced on subsequent runs)
4. `make seed` runs `scripts/seed-demo.sh` (creates demo org, users, teams with DB permissions)

## Default Credentials
| User | Password | Role |
|---|---|---|
| `admin@flowplane.local` | `Flowplane1!` | Platform admin (governance only) |
| `demo@acme-corp.com` | `Flowplane1!` | Demo user (org: acme-corp, team: engineering) — primary UI/CLI login |
| Zitadel admin | `zitadel-admin@zitadel.localhost` / `Password1!` | Zitadel console |

`make seed` also creates a machine agent (`flowplane-agent`) with client credentials for programmatic/MCP access. Run `make seed-info` to see all credentials.

## Verify
```bash
curl http://localhost:8080/api/v1/auth/mode
# {"auth_mode":"prod","oidc_issuer":"http://localhost:8081",...}

flowplane auth login      # Opens PKCE browser flow
flowplane auth whoami     # Show identity

curl http://localhost:8081  # Zitadel console
```

## Services
| Service | URL |
|---|---|
| API | http://localhost:8080 |
| UI | http://localhost:8080 |
| Swagger | http://localhost:8080/swagger-ui/ |
| Zitadel | http://localhost:8081 |
| xDS | localhost:50051 |
| Envoy | localhost:10000 (admin: 9901) |
| httpbin | localhost:8000 |
| MockBank | localhost:3001 (if `MOCKBACKEND=1`) |

## Optional Services
```bash
make up ENVOY=1              # + Envoy proxy
make up HTTPBIN=1            # + httpbin demo backend
make up MOCKBACKEND=1        # + MockBank API
make up-mtls                 # + Vault for mTLS
make up-tracing              # + Jaeger for tracing
make up-full                 # Everything
```

## Stop
```bash
make down    # Keep data
make clean   # Delete volumes + orphans (WARNING: also deletes .env.zitadel — requires full re-setup)
```

## Common Failures

**Image not found:** Run `make build` first.

**Zitadel connection refused:** Zitadel takes 30-60s on first boot. Wait and retry:
```bash
make setup-zitadel
```

**Port 8080 in use:** `make down` the previous stack first.

**Network conflict:** If previously ran `flowplane init`:
```bash
docker network rm flowplane-network
make up ENVOY=1 HTTPBIN=1
```

**xDS port:** Prod uses port **50051** (not 18000 like dev mode). Configure Envoy bootstrap accordingly.
