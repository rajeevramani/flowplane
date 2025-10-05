# HTTPBin Local Testing Guide

This guide walks through testing the x-flowplane filter extensions using the httpbin.org backend.

## Prerequisites

1. Flowplane control plane running locally (default: `http://localhost:8080`)
2. Envoy proxy configured and running
3. `curl` command-line tool

## Step 1: Import the OpenAPI Spec

```bash
cd examples

curl -X POST "http://localhost:8080/api/v1/api-definitions/from-openapi?team=demo&listenerIsolation=true" \
  -H "Content-Type: application/yaml" \
  --data-binary @httpbin-demo.yaml
```

Expected response:
```json
{
  "id": "abc-123-...",
  "bootstrapUri": "/api/v1/api-definitions/abc-123-.../bootstrap",
  "routes": ["route-id-1", "route-id-2", ...]
}
```

**Save the API ID** - you'll need it to get the bootstrap config.

## Step 2: Get the Bootstrap Config

Replace `{API_ID}` with the ID from Step 1:

```bash
curl "http://localhost:8080/api/v1/api-definitions/{API_ID}/bootstrap" \
  | jq > httpbin-bootstrap.json
```

## Step 3: Find the Assigned Port

The isolated listener was assigned a deterministic port based on the domain hash:

```bash
# Extract the listener port from bootstrap config
jq '.static_resources.listeners[] | select(.name | contains("platform")) | .address.socket_address.port_value' httpbin-bootstrap.json
```

Let's call this port `PORT` (likely in the 20000-29999 range).

## Step 4: Test the Endpoints

### Test 1: Simple GET with Global Filters

```bash
# Make a GET request
curl -v "http://localhost:${PORT}/get"
```

**What to look for:**
- Response includes `x-gateway: flowplane` header (added by global header mutation)
- Response includes `x-served-by: flowplane` header
- Response shows `x-forwarded-by: flowplane-gateway` was sent to httpbin

### Test 2: Check Headers Added by Gateway

```bash
curl "http://localhost:${PORT}/headers" | jq
```

**What to look for:**
```json
{
  "headers": {
    "X-Gateway": "flowplane",
    "X-Forwarded-By": "flowplane-gateway",
    ...
  }
}
```

### Test 3: POST with Route-Specific Rate Limit

```bash
# Make a POST request
curl "http://localhost:${PORT}/post" \
  -X POST \
  -H "Content-Type: application/json" \
  -d '{"test":"data","message":"from flowplane"}' \
  | jq
```

**What to look for:**
- Response includes `x-route-type: write-operation` header (route-specific)
- POST has a more restrictive rate limit (20/min vs 100/min global)

### Test 4: Test Rate Limiting

```bash
# Rapid fire requests to hit rate limit
for i in {1..25}; do
  echo "Request $i:"
  curl -s "http://localhost:${PORT}/post" \
    -X POST \
    -d '{"test":"'$i'"}' \
    -w "HTTP Status: %{http_code}\n" \
    -o /dev/null
  sleep 0.1
done
```

**What to look for:**
- First 20 requests: `HTTP Status: 200`
- After 20 requests: `HTTP Status: 429` (rate limited)

### Test 5: DELETE with Very Strict Rate Limit

```bash
# Try 15 DELETE requests (limit is 10/min)
for i in {1..15}; do
  echo "Request $i:"
  curl -s "http://localhost:${PORT}/delete" \
    -X DELETE \
    -w "HTTP Status: %{http_code}\n" \
    -o /dev/null
done
```

**What to look for:**
- First 10 requests: `HTTP Status: 200`
- Requests 11-15: `HTTP Status: 429`

### Test 6: Status Code Endpoint (Permissive Rate Limit)

```bash
# Test different status codes
curl -w "\nHTTP Status: %{http_code}\n" "http://localhost:${PORT}/status/200"
curl -w "\nHTTP Status: %{http_code}\n" "http://localhost:${PORT}/status/404"
curl -w "\nHTTP Status: %{http_code}\n" "http://localhost:${PORT}/status/500"
```

**What to look for:**
- Each returns the requested status code
- This endpoint has a permissive 500/min rate limit

### Test 7: Delay Endpoint (Testing Rate Limit)

```bash
# Request with 2 second delay
time curl "http://localhost:${PORT}/delay/2"
```

**What to look for:**
- Response takes ~2 seconds
- Strict rate limit: only 5 delay requests per minute

```bash
# Hit the delay rate limit
for i in {1..7}; do
  echo "Delay request $i:"
  curl -s "http://localhost:${PORT}/delay/1" \
    -w "HTTP Status: %{http_code}\n" \
    -o /dev/null &
done
wait
```

### Test 8: CORS Preflight Request

```bash
# Send an OPTIONS preflight request
curl -v "http://localhost:${PORT}/get" \
  -X OPTIONS \
  -H "Origin: http://example.com" \
  -H "Access-Control-Request-Method: GET"
```

**What to look for:**
- `Access-Control-Allow-Origin: *`
- `Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS`
- `Access-Control-Max-Age: 3600`

### Test 9: Custom Response Headers

```bash
curl -v "http://localhost:${PORT}/response-headers?foo=bar&baz=qux" | jq
```

**What to look for:**
- Response includes `x-custom-response: from-flowplane`
- Response includes custom headers set by httpbin

### Test 10: JSON Echo Test

```bash
curl "http://localhost:${PORT}/json" | jq
```

**What to look for:**
- Returns sample JSON from httpbin
- Includes gateway headers

## Step 5: Verify Filter Configuration

### Check Listener Config

```bash
jq '.static_resources.listeners[] | select(.name | contains("platform")) | .filter_chains[].filters[] | select(.name == "envoy.filters.network.http_connection_manager") | .typed_config.http_filters' httpbin-bootstrap.json
```

**What to look for:**
- CORS filter (`envoy.extensions.filters.http.cors.v3.Cors`)
- Header mutation filter (`envoy.extensions.filters.http.header_mutation.v3.HeaderMutation`)
- Rate limit filter (`envoy.extensions.filters.http.local_ratelimit.v3.LocalRateLimit`)

### Check Route Config

```bash
jq '.static_resources.listeners[] | select(.name | contains("platform")) | .filter_chains[].filters[] | select(.name == "envoy.filters.network.http_connection_manager") | .typed_config.route_config.virtual_hosts[].routes[]' httpbin-bootstrap.json
```

**What to look for:**
- Routes have `typed_per_filter_config` for route-specific overrides
- Different routes have different rate limit configs

## Troubleshooting

### Issue: "Connection refused"

**Solution:** Check that Envoy is running and listening on the assigned port:
```bash
netstat -an | grep ${PORT}
```

### Issue: "No route matched"

**Solution:** Check the routes in bootstrap config:
```bash
jq '.static_resources.listeners[] | select(.name | contains("platform")) | .filter_chains[].filters[] | select(.name == "envoy.filters.network.http_connection_manager") | .typed_config.route_config.virtual_hosts[].routes[].match' httpbin-bootstrap.json
```

### Issue: Filters not appearing

**Solution:** Verify you used `listenerIsolation=true` during import. Without it, filters won't be applied.

### Issue: Rate limiting not working

**Solution:**
1. Check Envoy logs for rate limit statistics
2. Verify `fill_interval_ms` is correct (milliseconds, not seconds)
3. Check if enough time has passed for bucket to refill

### Issue: CORS not working

**Solution:**
1. Verify preflight OPTIONS request is being sent
2. Check that Origin header is being sent
3. Verify CORS filter is in the http_filters list

## Clean Up

To remove the test API definition:

```bash
curl -X DELETE "http://localhost:8080/api/v1/api-definitions/{API_ID}"
```

## Advanced Testing

### Monitor Rate Limit Stats

If you have access to Envoy admin interface:

```bash
# Get stats for rate limiting
curl "http://localhost:19000/stats?filter=http_local_rate_limit"
```

### View Live Configuration

```bash
# Get current config from Envoy
curl "http://localhost:19000/config_dump" | jq > live-config.json
```

### Test Concurrent Requests

```bash
# Hammer the endpoint to test rate limiting under load
seq 1 100 | xargs -P 10 -I {} curl -s "http://localhost:${PORT}/get" -w "%{http_code}\n" -o /dev/null
```

## Example Output

### Successful GET Request
```json
{
  "args": {},
  "headers": {
    "Accept": "*/*",
    "Host": "httpbin.org",
    "User-Agent": "curl/7.81.0",
    "X-Forwarded-By": "flowplane-gateway",
    "X-Gateway": "flowplane"
  },
  "origin": "...",
  "url": "https://httpbin.org/get"
}
```

### Rate Limited Response
```
HTTP/1.1 429 Too Many Requests
x-envoy-ratelimited: true
```

### CORS Preflight Response
```
HTTP/1.1 200 OK
access-control-allow-origin: *
access-control-allow-methods: GET, POST, PUT, DELETE, OPTIONS
access-control-allow-headers: content-type, authorization, x-api-key, x-custom-header
access-control-max-age: 3600
```
