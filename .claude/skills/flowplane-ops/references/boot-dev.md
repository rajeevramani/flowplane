# Dev Mode Boot Recipe

## Prerequisites
- Docker or Podman running
- Flowplane images built (`make build`)

## Boot
```bash
flowplane init --with-envoy --with-httpbin
```

## What Happens
1. Detects container runtime (Docker/Podman)
2. Generates 43-char base64 dev token
3. Writes compose file to `~/.flowplane/`
4. Starts: PostgreSQL, control plane, Envoy (optional), httpbin (optional)
5. Waits for `/health` (60s timeout)
6. Saves token to `~/.flowplane/credentials`
7. Auto-seeds: org `dev-org`, team `default`, user `dev@flowplane.local`, dataplane `dev-dataplane`

## Verify
```bash
curl http://localhost:8080/api/v1/auth/mode
# {"auth_mode":"dev"}

flowplane status
flowplane auth whoami
# dev@flowplane.local, org: dev-org, team: default

curl http://localhost:10000/    # Envoy (if --with-envoy)
curl http://localhost:8000/get  # httpbin (if --with-httpbin)
```

## Services
| Service | URL |
|---|---|
| API | http://localhost:8080 |
| Swagger | http://localhost:8080/swagger-ui/ |
| xDS | localhost:18000 |
| Envoy | localhost:10000 (admin: 9901) |
| httpbin | localhost:8000 |

## Stop
```bash
flowplane down           # Keep data
flowplane down --volumes # Delete data
```

## Common Failures

**Image not found:** Run `make build` first.

**Port 8080 in use:** `flowplane down` or `make down` the previous stack.

**Network conflict:** If previously ran `make up`, remove the network:
```bash
docker network rm flowplane-network
flowplane init --with-envoy --with-httpbin
```

**Loopback guard:** `flowplane init` refuses to run inside a container. Use `make up` instead if you're in a devcontainer.
