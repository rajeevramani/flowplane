# Quickstart

Expose a local HTTP service through an Envoy proxy with rate limiting in under 10 minutes.

## Prerequisites

- **Docker** (or Podman — auto-detected)
- **Rust toolchain** (for building from source) — [install via rustup](https://rustup.rs/)

## 1. Install and start

```bash
git clone https://github.com/rajeevramani/flowplane.git
cd flowplane
cargo install --path . --locked
flowplane init --with-envoy --with-httpbin
```

This starts PostgreSQL, the Flowplane control plane, an Envoy proxy, and an httpbin test backend. Dev mode — no login required.

| Service | URL |
|---------|-----|
| API | http://localhost:8080/api/v1/ |
| UI | http://localhost:8080/ |
| Envoy | ports 10001-10020 (auto-assigned) |
| httpbin | http://localhost:8000 (direct, not through Envoy) |

## 2. Expose httpbin

```bash
flowplane expose http://httpbin:80 --name httpbin-service
```

This creates a cluster, route config, listener, and virtual host. Envoy picks up the configuration via xDS.

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

The `X-Envoy-Expected-Rq-Timeout-Ms` header confirms traffic flows through Envoy.

## 4. Add a rate limit filter

Create a filter spec file:

```bash
cat > /tmp/ratelimit.json << 'EOF'
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
```

Create the filter and attach it to the listener:

```bash
flowplane filter create -f /tmp/ratelimit.json
flowplane filter attach demo-rate-limit --listener httpbin-service-listener --order 1
```

## 5. Test the rate limit

Send a few rapid requests:

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

Requests beyond the token bucket limit receive `429 Too Many Requests`.

## 6. Explore

```bash
flowplane list      # see exposed services
flowplane status    # system health
flowplane doctor    # diagnostic checks
```

Open http://localhost:8080 for the web UI. In dev mode no login is required.

Browse the REST API at http://localhost:8080/swagger-ui/.

## Cleanup

```bash
flowplane down             # stop containers, keep data
flowplane down --volumes   # stop and delete all data
```

## Production Mode

For multi-user deployments with Zitadel authentication:

```bash
make up HTTPBIN=1 ENVOY=1
make seed
flowplane auth login
```

`make seed` creates the `acme-corp` demo org with credentials: `demo@acme-corp.com` / `Flowplane1!`.

## Next steps

- [Getting Started](getting-started.md) — full walkthrough with MCP examples
- [CLI Reference](cli-reference.md) — every command, flag, and example
- [Filters](filters.md) — all 10 filter types and configuration
- [MCP Tools](mcp.md) — 68 tools for AI-driven gateway management
