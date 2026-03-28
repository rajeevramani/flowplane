# Quickstart

Expose a local HTTP service through an Envoy proxy with rate limiting in under 10 minutes.

## Prerequisites

- **Docker** (or Podman — auto-detected)
- **git**
- **Rust toolchain** (for building from source) — [install via rustup](https://rustup.rs/)

## 1. Install and start

```bash
git clone <repo-url> flowplane
cd flowplane
cargo install --path .
flowplane init --with-envoy --with-httpbin
```

This starts PostgreSQL, the Flowplane control plane, an Envoy proxy, and an httpbin test backend. Dev mode — no login required.

| Service | URL |
|---------|-----|
| API | http://localhost:8080/api/v1/ |
| UI | http://localhost:8080/ |
| Envoy | http://localhost:10000 |
| httpbin | http://localhost:8000 |

## 2. Expose httpbin

```bash
flowplane expose http://httpbin:80 --name httpbin-service
```

This single command creates a cluster, route config, listener, and virtual host. Envoy picks up the new configuration via xDS.

Output:

```
Exposed 'httpbin-service' -> http://httpbin:80
  Port:   10001
  Paths:  /

  curl http://localhost:10001/
```

## 3. Verify traffic flows

```bash
curl http://localhost:10001/get
```

You should see a JSON response from httpbin:

```json
{
  "args": {},
  "headers": {
    "Accept": "*/*",
    "Host": "localhost:10001",
    "User-Agent": "curl/8.x"
  },
  "origin": "172.17.0.1",
  "url": "http://localhost:10001/get"
}
```

## 4. Add a rate limit filter

Create a filter spec file:

```bash
cat > /tmp/ratelimit.json << 'EOF'
{
  "name": "demo-rate-limit",
  "filterType": "local_rate_limit",
  "description": "Allow 3 requests per minute",
  "configuration": {
    "stat_prefix": "demo_rate_limit",
    "token_bucket": {
      "max_tokens": 3,
      "tokens_per_fill": 3,
      "fill_interval": "60s"
    },
    "filter_enabled": {
      "runtime_key": "local_rate_limit_enabled",
      "default_value": {
        "numerator": 100,
        "denominator": "HUNDRED"
      }
    },
    "filter_enforced": {
      "runtime_key": "local_rate_limit_enforced",
      "default_value": {
        "numerator": 100,
        "denominator": "HUNDRED"
      }
    }
  }
}
EOF
```

Create the filter and attach it to the listener:

```bash
flowplane filter create --file /tmp/ratelimit.json
flowplane filter attach demo-rate-limit --listener httpbin-service-listener
```

## 5. Test the rate limit

Send a few rapid requests:

```bash
for i in $(seq 1 5); do
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

Requests beyond the token bucket limit receive `429 Too Many Requests`.

## 6. Explore the UI

Open http://localhost:8080. In dev mode no login is required — the dashboard shows the cluster, listener, routes, and attached filters you just created.

## Cleanup

Stop all services:

```bash
flowplane down
```

To remove volumes and start fresh:

```bash
flowplane down --volumes
```

## Production Mode

For multi-user deployments, `make up` boots the full stack with Zitadel for authentication and multi-tenant isolation.

```bash
make up HTTPBIN=1 ENVOY=1
make seed
flowplane auth login
```

`make seed` creates the `acme-corp` demo org with credentials: `demo@acme-corp.com` / `Flowplane1!`.

See [CLI Reference](cli-reference.md) for the full command documentation.

## Next steps

- [CLI Reference](cli-reference.md) — full command documentation
- [Filters](filters.md) — all supported filter types and configuration
- [MCP Tools](mcp.md) — programmatic control via Model Context Protocol
