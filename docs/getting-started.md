# Getting Started

This guide walks you through setting up Flowplane in dev mode and exposing your first service through Envoy.

## Prerequisites

- **Docker** (Docker Desktop, OrbStack, Rancher Desktop, or Podman)
- **Rust** (stable, 1.75+) with cargo

## Install

```bash
git clone https://github.com/rajeevramani/flowplane.git
cd flowplane
cargo install --path . --locked
```

This installs the `flowplane` CLI binary.

## Boot the stack

```bash
flowplane init --with-envoy --with-httpbin
```

This starts PostgreSQL, the Flowplane control plane, Envoy, and httpbin in Docker containers. A dev token is generated and saved to `~/.flowplane/credentials` automatically.

Services after boot:

| Service | Address |
|---|---|
| Control plane API | http://localhost:8080 |
| Swagger UI | http://localhost:8080/swagger-ui/ |
| xDS server | localhost:18000 (gRPC) |
| Envoy proxy | localhost:10000 (admin), ports 10001-10020 (listeners) |
| httpbin | http://localhost:8000 |

> Envoy listeners use ports **10001-10020**. The `expose` command auto-assigns the next available port in this range.

## Verify

```bash
flowplane status
curl http://localhost:8080/health
```

`flowplane status` shows a system overview (control plane, Envoy, database). The `/health` endpoint returns `200 OK` when the control plane is ready.

## Expose a service

The `expose` command creates all the gateway resources (cluster, route, listener) in one step:

```bash
flowplane expose http://httpbin:80 --name demo
```

```
Exposed 'demo' -> http://httpbin:80
  Port:   10001
  Paths:  /

  curl http://localhost:10001/
```

This created a cluster pointing to `httpbin:80`, a route config forwarding `/` to that cluster, and a listener on port `10001`. The port is auto-assigned from the range 10001-10020.

### Test it

Send a request through the Envoy gateway:

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
  "origin": "10.89.0.13",
  "url": "http://localhost:10001/get"
}
```

The `X-Envoy-Expected-Rq-Timeout-Ms` header confirms the request went through Envoy.

Try other httpbin endpoints:

```bash
curl http://localhost:10001/status/200    # Returns 200 OK
curl http://localhost:10001/headers       # Echo request headers
curl -X POST http://localhost:10001/post -d '{"key":"value"}'  # Echo POST body
```

### Check what's exposed

```bash
flowplane list
```

```
Name                           Port     Protocol
-------------------------------------------------------
demo                           10001    HTTP
```

```bash
flowplane status
```

```
Flowplane Status (team: default)
----------------------------------------
Listeners:  1
Clusters:   1
Filters:    0
```

### Expose options

| Flag | Description |
|---|---|
| `--name <NAME>` | Service name (auto-generated from URL if omitted) |
| `--path <PATH>` | Path prefix to route (repeatable, defaults to `/`) |
| `--port <PORT>` | Port override (auto-assigned from 10001-10020 if omitted) |

### Remove an exposed service

```bash
flowplane unexpose demo
```

```
Removed exposed service 'demo'
```

This tears down the listener, route config, and cluster in one step.

## Add a rate limit filter

Flowplane filters let you add behavior to your gateway without changing upstream code. This section adds local rate limiting to the `demo` service.

First, re-expose the service if you removed it:

```bash
flowplane expose http://httpbin:80 --name demo
```

### Create the filter

Save this as `rate-limit-filter.json`:

```json
{
  "name": "rate-limit",
  "filterType": "local_rate_limit",
  "config": {
    "type": "local_rate_limit",
    "config": {
      "stat_prefix": "http_local_rate_limiter",
      "token_bucket": {
        "max_tokens": 3,
        "tokens_per_fill": 3,
        "fill_interval_ms": 60000
      },
      "filter_enabled": {
        "numerator": 100,
        "denominator": "hundred"
      },
      "filter_enforced": {
        "numerator": 100,
        "denominator": "hundred"
      }
    }
  },
  "team": "default"
}
```

Key fields:
- `filterType` — the filter type (`local_rate_limit` for in-memory rate limiting)
- `config` — nested structure with `type` and `config` (the inner config holds filter-specific settings)
- `token_bucket` — allows 3 requests per 60 seconds, then rejects with 429
- `filter_enabled` / `filter_enforced` — both set to 100% so every request is checked and enforced

Create the filter:

```bash
flowplane filter create -f rate-limit-filter.json
```

```json
{
  "id": "e8169ce5-f561-4deb-ae77-86bf26d4a4f5",
  "name": "rate-limit",
  "filterType": "local_rate_limit",
  "version": 1,
  "team": "default",
  "allowedAttachmentPoints": ["route", "listener"]
}
```

### Attach to the listener

Filters are not active until attached to a listener (or route):

```bash
flowplane filter attach rate-limit --listener demo-listener
```

```
Filter 'rate-limit' attached to listener 'demo-listener'
```

> The listener name follows the `expose` naming convention: `<name>-listener`. Since we used `--name demo`, the listener is `demo-listener`.

### Test rate limiting

Send five requests in quick succession:

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

The first three requests succeed (consuming all tokens). Requests 4 and 5 are rejected with `429 Too Many Requests` and the body `local_rate_limited`. The bucket refills after 60 seconds.

### Verify filter status

```bash
flowplane filter list
```

```
Name                           Type                 Team            Version    Attached  
------------------------------------------------------------------------------------------
rate-limit                     local_rate_limit     default         1          1
```

### Clean up the filter

To remove rate limiting, detach the filter from the listener, then delete it:

```bash
flowplane filter detach rate-limit --listener demo-listener
flowplane filter delete rate-limit --yes
```

```
Filter 'rate-limit' detached from listener 'demo-listener'
Filter 'rate-limit' deleted successfully
```
