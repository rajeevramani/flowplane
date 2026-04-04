# Getting Started

Expose a local service through Envoy with rate limiting in under 10 minutes.

## Prerequisites

- **Docker** (or Podman)
- **Rust 1.92+** — install via [rustup.rs](https://rustup.rs/)

## 1. Install

```bash
git clone https://github.com/rajeevramani/flowplane.git
cd flowplane
cargo install --path . --locked
```

This builds the `flowplane` CLI binary. The next step pulls Docker images automatically.

## 2. Boot the stack

```bash
flowplane init --with-envoy --with-httpbin
```

This starts four containers in dev mode — no login required:

| Service    | Address                           |
|------------|-----------------------------------|
| API        | http://localhost:8080              |
| Swagger UI | http://localhost:8080/swagger-ui/  |
| httpbin    | http://localhost:8000              |
| Envoy      | localhost:10000 (base)            |

A dev token is generated and saved to `~/.flowplane/credentials`. All CLI commands use it automatically.

> ⚠️ If you previously ran `make up` (prod mode), remove the stale network first: `docker network rm flowplane-network`

## 3. Verify the stack

```bash
flowplane status
```

```
Flowplane Status (team: default)
----------------------------------------
Listeners:  0
Clusters:   0
Filters:    0
```

Confirm dev mode:

```bash
curl http://localhost:8080/api/v1/auth/mode
```

```json
{"auth_mode":"dev"}
```

## 4. Expose httpbin

<table>
<tr><th>CLI</th><th>MCP</th></tr>
<tr>
<td>

```bash
flowplane expose http://httpbin:80 \
  --name demo
```

Output:

```
Exposed 'demo' -> http://httpbin:80
  Port:   10001
  Paths:  /

  curl http://localhost:10001/
```

</td>
<td>

Three tool calls via `POST /api/v1/mcp`:

**1. Create cluster:**

```json
{
  "method": "tools/call",
  "params": {
    "name": "cp_create_cluster",
    "arguments": {
      "name": "demo",
      "serviceName": "demo-service",
      "endpoints": [{"address": "httpbin", "port": 80}],
      "team": "default"
    }
  }
}
```

**2. Create route config:**

```json
{
  "method": "tools/call",
  "params": {
    "name": "cp_create_route_config",
    "arguments": {
      "name": "demo-routes",
      "virtualHosts": [{
        "name": "demo-vhost",
        "domains": ["*"],
        "routes": [{
          "name": "catch-all",
          "match": {"path": {"type": "prefix", "value": "/"}},
          "action": {"type": "forward", "cluster": "demo"}
        }]
      }],
      "team": "default"
    }
  }
}
```

**3. Create listener** (get `dataplaneId` from `cp_list_dataplanes` first):

```json
{
  "method": "tools/call",
  "params": {
    "name": "cp_create_listener",
    "arguments": {
      "name": "demo-listener",
      "address": "0.0.0.0",
      "port": 10001,
      "routeConfigName": "demo-routes",
      "dataplaneId": "<from cp_list_dataplanes>",
      "team": "default"
    }
  }
}
```

</td>
</tr>
</table>

> ⚠️ Use the Docker service name (`httpbin`), not `localhost`. Inside the Docker network, `localhost` refers to the container itself.

> ⚠️ **MCP in dev mode:** Always include `"team": "default"` in every tool call. The dev auth context has no grants, so team resolution fails without it.

## 5. Test with curl

```bash
curl http://localhost:10001/get
```

```json
{
  "args": {},
  "headers": {
    "Accept": "*/*",
    "Host": "localhost:10001",
    "User-Agent": "curl/8.7.1",
    "X-Envoy-Expected-Rq-Timeout-Ms": "15000"
  },
  "origin": "10.89.0.5",
  "url": "http://localhost:10001/get"
}
```

The `X-Envoy-Expected-Rq-Timeout-Ms` header confirms the request went through Envoy.

> ⚠️ The port is auto-assigned from the 10001–10020 range. On a fresh stack, the first expose gets 10001. Always check the `expose` output for the actual port.

## 6. Add a rate limit filter

<table>
<tr><th>CLI</th><th>MCP</th></tr>
<tr>
<td>

```bash
cat > /tmp/rl-filter.json <<'EOF'
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

flowplane filter create -f /tmp/rl-filter.json
flowplane filter attach demo-rate-limit \
  --listener demo-listener --order 1
```

</td>
<td>

**1. Create filter:**

```json
{
  "method": "tools/call",
  "params": {
    "name": "cp_create_filter",
    "arguments": {
      "name": "demo-rate-limit",
      "filterType": "local_rate_limit",
      "configuration": {
        "type": "local_rate_limit",
        "config": {
          "stat_prefix": "demo_rl",
          "token_bucket": {
            "max_tokens": 3,
            "tokens_per_fill": 3,
            "fill_interval_ms": 60000
          }
        }
      },
      "team": "default"
    }
  }
}
```

**2. Attach to listener:**

```json
{
  "method": "tools/call",
  "params": {
    "name": "cp_attach_filter",
    "arguments": {
      "filter": "demo-rate-limit",
      "listener": "demo-listener",
      "order": 1,
      "team": "default"
    }
  }
}
```

</td>
</tr>
</table>

> ⚠️ The filter `config` field uses nested `{type, config}` — not a flat structure. The inner `type` must match the `filterType` field.

## 7. Verify rate limiting

```bash
for i in 1 2 3 4 5; do
  echo "Request $i: $(curl -s -o /dev/null -w '%{http_code}' http://localhost:10001/get)"
done
```

```
Request 1: 200
Request 2: 200
Request 3: 200
Request 4: 429
Request 5: 429
```

The token bucket allows 3 requests per 60-second window. Requests 4 and 5 get `429 Too Many Requests`.

## 8. Explore

```bash
flowplane list                           # See exposed services
flowplane status                         # System health overview
flowplane doctor                         # Run diagnostic checks
```

Browse the full REST API at http://localhost:8080/swagger-ui/.

### MCP connection

Flowplane exposes 60+ MCP tools at `POST /api/v1/mcp`. To connect from Claude Code or another MCP client:

```bash
# Get your dev token
flowplane auth token
```

Required headers:

```
Authorization: Bearer <token>
MCP-Protocol-Version: 2025-11-25
```

After the `initialize` handshake, include the `MCP-Session-Id` header from the response on all subsequent requests.

## 9. Tear down

```bash
flowplane down             # Stop containers, keep data
flowplane down --volumes   # Stop and delete all data
```

---

## Advanced: Production Mode

Production mode adds Zitadel for multi-tenant authentication with OIDC.

```bash
make build                        # Build images (first time only)
make up ENVOY=1 HTTPBIN=1         # Start full stack with Zitadel
make seed                         # Create demo org and credentials
make seed-info                    # Print login credentials
```

Default login: `demo@acme-corp.com` / `Flowplane1!`

```bash
flowplane auth login              # Opens browser-based PKCE flow
```

| Setting      | Dev                  | Prod                                |
|--------------|----------------------|-------------------------------------|
| Auth         | Auto dev token       | Zitadel PKCE (`flowplane auth login`) |
| xDS port     | 18000                | 50051                               |
| Zitadel      | Not running          | localhost:8081                      |
| Multi-tenant | Single `default` team | Multiple orgs and teams             |

---

## Next steps

- [CLI Reference](cli-reference.md) — every command, flag, and example
- [Filters](filters.md) — rate limiting, JWT auth, CORS, and 11 more filter types
- [MCP Server](mcp.md) — full tool catalog for AI-driven gateway management
