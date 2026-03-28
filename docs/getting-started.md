# Getting Started

Expose a local service through Envoy with rate limiting in under 10 minutes.

## Prerequisites

- **Docker** (or Podman)
- **Rust 1.92+** — needed for `cargo install` ([rustup.rs](https://rustup.rs/))

## 1. Install

```bash
git clone https://github.com/rajeevramani/flowplane.git
cd flowplane
cargo install --path .
```

`cargo install` builds the `flowplane` CLI binary. The `flowplane init` command (next step) pulls the required Docker images automatically.

## 2. Boot the stack

```bash
flowplane init --with-envoy --with-httpbin
```

This starts four services in dev mode — no Zitadel, no login required. A dev token is auto-saved to `~/.flowplane/credentials`.

| Service    | Address                              |
|------------|--------------------------------------|
| API        | http://localhost:8080                |
| Envoy      | localhost:10000                      |
| httpbin    | localhost:8000                       |
| Swagger UI | http://localhost:8080/swagger-ui/    |

## 3. Verify

```bash
flowplane status
```

This shows listener, cluster, and filter counts.

Confirm dev mode is active:

```bash
curl http://localhost:8080/api/v1/auth/mode
```

Expected response:

```json
{"auth_mode":"dev"}
```

## 4. Expose httpbin

**CLI:**

```bash
flowplane expose http://httpbin:80 --name demo
```

> Use the Docker service name (`httpbin`), not `localhost`. The control plane resolves names within the Docker network.

**MCP** (via `POST /api/v1/mcp` — see [Section 8](#8-explore) for connection setup):

Step 1 — Create the cluster:

```json
{"method": "tools/call", "params": {"name": "cp_create_cluster", "arguments": {
  "name": "demo",
  "endpoints": ["httpbin:80"],
  "lb_policy": "ROUND_ROBIN"
}}}
```

Step 2 — Create a route config pointing to the cluster:

```json
{"method": "tools/call", "params": {"name": "cp_create_route_config", "arguments": {
  "name": "demo-routes",
  "virtualHosts": [{
    "name": "demo-vhost",
    "domains": ["*"],
    "routes": [{
      "name": "catch-all",
      "match": {"path": {"type": "prefix", "value": "/"}},
      "action": {"type": "forward", "cluster": "demo"}
    }]
  }]
}}}
```

Step 3 — Create a listener (requires a `dataplaneId` — get it from `cp_list_dataplanes`):

```json
{"method": "tools/call", "params": {"name": "cp_create_listener", "arguments": {
  "name": "demo-listener",
  "address": "0.0.0.0",
  "port": 10001,
  "dataplaneId": "<from cp_list_dataplanes>"
}}}
```

> Port must be in the 10001–10020 range. Use `cp_query_port` to check availability.

## 5. Test

The `expose` command prints the assigned port. Use it in the curl:

```bash
curl http://localhost:<PORT>/get
```

> On a fresh stack the first expose gets port 10001. If other services are already exposed, the port may differ — check the `expose` output.

Expected response (truncated):

```json
{
  "args": {},
  "headers": {
    "Accept": "*/*",
    "Host": "localhost:10001",
    "User-Agent": "curl/8.x"
  },
  "origin": "172.x.x.x",
  "url": "http://localhost:10001/get"
}
```

## 6. Add a rate limit filter

**CLI:**

```bash
cat > filter.json <<'EOF'
{
  "name": "demo-rate-limit",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "demo_rl",
      "token_bucket": {
        "max_tokens": 3,
        "tokens_per_fill": 3,
        "fill_interval_ms": 60000
      }
    }
  }
}
EOF

flowplane filter create -f filter.json
flowplane filter attach demo-rate-limit --listener demo-listener --order 1
```

**MCP:**

```json
// 1. Create the filter
{
  "method": "tools/call",
  "params": {
    "name": "cp_create_filter",
    "arguments": {
      "name": "demo-rate-limit",
      "filterType": "local_rate_limit",
      "config": {
        "type": "local_rate_limit",
        "config": {
          "stat_prefix": "demo_rl",
          "token_bucket": {
            "max_tokens": 3,
            "tokens_per_fill": 3,
            "fill_interval_ms": 60000
          }
        }
      }
    }
  }
}

// 2. Attach to the listener
{
  "method": "tools/call",
  "params": {
    "name": "cp_attach_filter",
    "arguments": {
      "listener": "demo-listener",
      "filter": "demo-rate-limit",
      "order": 1
    }
  }
}
```

## 7. Test rate limiting

```bash
PORT=10001  # use the port from `flowplane expose` output
for i in 1 2 3 4 5; do
  echo "Request $i: $(curl -s -o /dev/null -w '%{http_code}' http://localhost:$PORT/get)"
done
```

Expected output:

```
Request 1: 200
Request 2: 200
Request 3: 200
Request 4: 429
Request 5: 429
```

The bucket allows 3 requests per 60-second window. Requests 4 and 5 are rejected with `429 Too Many Requests`.

## 8. Explore

- **List exposed services:** `flowplane list`
- **Swagger UI:** browse the full REST API at http://localhost:8080/swagger-ui/
- **MCP endpoint:** `POST /api/v1/mcp` (Streamable HTTP, protocol version `2025-11-25`)

To connect an MCP client:

```bash
# Get your dev token
flowplane auth token
```

Required headers for MCP requests:

```
Authorization: Bearer <token>
MCP-Protocol-Version: 2025-11-25
```

## 9. Tear down

```bash
# Stop containers
flowplane down

# Stop and remove volumes (deletes all data)
flowplane down --volumes
```

## Advanced: Production Mode

Use production mode when you need multi-tenant isolation and real authentication.

```bash
# Boot the full stack (includes Zitadel)
make up ENVOY=1 HTTPBIN=1

# Seed demo data
make seed

# Show credentials
make seed-info
```

Default credentials: `demo@acme-corp.com` / `Flowplane1!`

Key differences from dev mode:

| Setting       | Dev                | Prod                          |
|---------------|--------------------|-------------------------------|
| Auth          | Auto dev token     | Zitadel PKCE (`flowplane auth login`) |
| xDS port      | 18000              | 50051                         |
| Zitadel       | Not running        | localhost:8081                 |
| Multi-tenant  | No                 | Yes                           |

```bash
# Login opens a browser-based PKCE flow
flowplane auth login
```

## Next steps

- [CLI Reference](cli-reference.md) — full command documentation
- [Filters](filters.md) — all filter types and configuration options
- [MCP](mcp.md) — tool catalog and protocol details
