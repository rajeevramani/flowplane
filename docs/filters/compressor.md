# Compressor Filter

The Compressor filter compresses HTTP response bodies to reduce bandwidth usage and improve client performance. It supports multiple compression algorithms (gzip, Brotli, Zstandard) and can be configured based on content type, response size, and client capabilities.

## Envoy Documentation

- [Compressor Filter Reference](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/compressor_filter)
- [Compressor Filter API](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/filters/http/compressor/v3/compressor.proto)
- [Gzip Compressor](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/compression/gzip/compressor/v3/gzip.proto)
- [Brotli Compressor](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/compression/brotli/compressor/v3/brotli.proto)

## How It Works in Envoy

The Compressor filter inspects responses and compresses them based on client capabilities and configuration:

```
┌─────────┐     ┌─────────────────────────────────────────┐     ┌─────────────┐
│  Client │     │                 Envoy                   │     │  Upstream   │
└────┬────┘     │  ┌─────────────────────────────────┐   │     └──────┬──────┘
     │          │  │       Compressor Filter         │   │            │
     │          │  │                                 │   │            │
     │          │  │  1. Check Accept-Encoding       │   │            │
     │          │  │  2. Check Content-Type match    │   │            │
     │          │  │  3. Check min content length    │   │            │
     │          │  │  4. Compress if conditions met  │   │            │
     │          │  └─────────────────────────────────┘   │            │
     │          └────────────────────────────────────────┘            │
     │               │                                                │
     │ 1. Request    │                                                │
     │ Accept-Encoding: gzip                                          │
     ├──────────────►│                                                │
     │               │                                                │
     │               │ 2. Forward request ───────────────────────────►│
     │               │                                                │
     │               │ 3. Response (uncompressed) ◄───────────────────┤
     │               │    Content-Type: application/json              │
     │               │    Body: 5KB JSON                              │
     │               │                                                │
     │               │ 4. Compress response                           │
     │               │    (gzip algorithm)                            │
     │               │                                                │
     │ 5. Response   │                                                │
     │ Content-Encoding: gzip                                         │
     │ Body: ~1KB compressed                                          │
     │◄──────────────┤                                                │
```

### Key Behaviors

1. **Client Negotiation**: Only compresses if client sends `Accept-Encoding` header with supported algorithm
2. **Content-Type Filtering**: Compresses only configured content types (e.g., JSON, HTML, text)
3. **Minimum Size Threshold**: Skips compression for small responses where overhead outweighs benefit
4. **ETag Handling**: Can optionally skip compression for responses with ETag headers
5. **Vary Header**: Automatically adds `Vary: Accept-Encoding` to compressed responses
6. **Multiple Algorithms**: Supports gzip, Brotli, and Zstandard with algorithm-specific tuning

### Per-Route Support

**The Compressor filter supports per-route configuration** via `typed_per_filter_config`. You can:
- Disable compression for specific routes (e.g., already-compressed content like images)
- Override compression settings per route or virtual host

## Flowplane Configuration

### Filter Definition

Create a reusable filter definition with gzip compression:

```json
{
  "name": "compression-filter",
  "filterType": "compressor",
  "description": "Compression filter - gzip for JSON/HTML/text responses",
  "config": {
    "type": "compressor",
    "config": {
      "response_direction_config": {
        "common_config": {
          "min_content_length": 100,
          "content_type": [
            "application/json",
            "text/html",
            "text/plain"
          ],
          "disable_on_etag_header": false,
          "remove_accept_encoding_header": false
        }
      },
      "compressor_library": {
        "type": "gzip",
        "compression_level": "best_speed",
        "compression_strategy": "default_strategy",
        "memory_level": 5,
        "window_bits": 12,
        "chunk_size": 4096
      }
    }
  },
  "team": "my-team"
}
```

### Top-Level Configuration Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `response_direction_config` | object | No | - | Configuration for response compression |
| `request_direction_config` | object | No | - | Configuration for request compression (rarely used) |
| `compressor_library` | object | Yes | - | Compression algorithm configuration |
| `choose_first` | boolean | No | `false` | Choose first acceptable algorithm instead of best |

### Response Direction Config

Controls when and how responses are compressed:

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `common_config` | object | No | - | Common compression settings |

### Common Config

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `enabled` | object | No | 100% | Percentage of requests to compress |
| `min_content_length` | integer | No | `30` | Minimum response size to compress (bytes) |
| `content_type` | array | No | See note | Content types to compress |
| `disable_on_etag_header` | boolean | No | `false` | Skip compression for responses with ETag |
| `remove_accept_encoding_header` | boolean | No | `false` | Remove Accept-Encoding before forwarding |

**Default Content Types** (if not specified):
- `text/html`
- `text/plain`
- `text/css`
- `application/javascript`
- `application/x-javascript`
- `text/javascript`
- `text/x-javascript`
- `text/ecmascript`
- `text/js`
- `text/jscript`
- `text/x-js`
- `application/ecmascript`
- `application/x-json`
- `application/xml`
- `application/json`

### Compressor Library Configuration

#### Gzip Compressor

```json
{
  "compressor_library": {
    "type": "gzip",
    "compression_level": "best_speed",
    "compression_strategy": "default_strategy",
    "memory_level": 5,
    "window_bits": 12,
    "chunk_size": 4096
  }
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `memory_level` | integer | No | `5` | Memory usage (1-9, higher = more memory, better compression) |
| `compression_level` | string | No | `"default_compression"` | Compression level |
| `compression_strategy` | string | No | `"default_strategy"` | Compression strategy |
| `window_bits` | integer | No | `12` | Window size (9-15, higher = better compression) |
| `chunk_size` | integer | No | `4096` | Output chunk size in bytes |

**Compression Levels:**
- `"default_compression"` - Balance of speed and compression
- `"best_speed"` - Fastest compression, lower ratio
- `"best_compression"` - Best ratio, slower
- `"compression_level_1"` through `"compression_level_9"` - Specific levels

**Compression Strategies:**
- `"default_strategy"` - General purpose
- `"filtered"` - For data with small random values
- `"huffman_only"` - Huffman encoding only
- `"rle"` - Run-length encoding
- `"fixed"` - Fixed Huffman codes

#### Brotli Compressor

```json
{
  "compressor_library": {
    "type": "brotli",
    "quality": 4,
    "window_bits": 18,
    "input_block_bits": 24,
    "chunk_size": 4096,
    "disable_literal_context_modeling": false
  }
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `quality` | integer | No | `4` | Compression quality (0-11, higher = better ratio) |
| `window_bits` | integer | No | `18` | Window size bits (10-24) |
| `input_block_bits` | integer | No | `24` | Input block size bits (16-24) |
| `chunk_size` | integer | No | `4096` | Output chunk size in bytes |
| `disable_literal_context_modeling` | boolean | No | `false` | Disable context modeling for faster compression |

#### Zstandard Compressor

```json
{
  "compressor_library": {
    "type": "zstd",
    "compression_level": 3,
    "enable_checksum": false,
    "strategy": "default",
    "chunk_size": 4096
  }
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `compression_level` | integer | No | `3` | Compression level (1-22) |
| `enable_checksum` | boolean | No | `false` | Add checksum for integrity |
| `strategy` | string | No | `"default"` | Compression strategy |
| `chunk_size` | integer | No | `4096` | Output chunk size in bytes |

## Filter Installation Workflow

### Step 1: Create the Filter Definition

```bash
curl -X POST http://localhost:8080/api/v1/filters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "compression-test-filter",
    "filterType": "compressor",
    "description": "Compression filter - gzip for JSON/HTML/text responses",
    "config": {
      "type": "compressor",
      "config": {
        "response_direction_config": {
          "common_config": {
            "min_content_length": 100,
            "content_type": [
              "application/json",
              "text/html",
              "text/plain"
            ],
            "disable_on_etag_header": false,
            "remove_accept_encoding_header": false
          }
        },
        "compressor_library": {
          "type": "gzip",
          "compression_level": "best_speed",
          "compression_strategy": "default_strategy",
          "memory_level": 5,
          "window_bits": 12,
          "chunk_size": 4096
        }
      }
    },
    "team": "my-team"
  }'
```

### Step 2: Create Supporting Resources

Create the cluster:

```bash
curl -X POST http://localhost:8080/api/v1/clusters \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "compression-test-cluster",
    "team": "my-team",
    "connectTimeout": "5s",
    "loadBalancingPolicy": "round_robin",
    "endpoints": [
      {"host": "localhost", "port": 8001}
    ]
  }'
```

Create the route configuration:

```bash
curl -X POST http://localhost:8080/api/v1/route-configs \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "compression-test-routes",
    "team": "my-team",
    "virtualHosts": [{
      "name": "compression-test-vhost",
      "domains": ["compression-test.local"],
      "routes": [{
        "name": "json-route",
        "match": {"path": {"type": "prefix", "value": "/testing/compression"}},
        "action": {
          "type": "forward",
          "cluster": "compression-test-cluster",
          "timeoutSeconds": 30,
          "prefixRewrite": "/json"
        }
      }]
    }]
  }'
```

Create the listener:

```bash
curl -X POST http://localhost:8080/api/v1/listeners \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "compression-test-listener",
    "address": "0.0.0.0",
    "port": 10093,
    "team": "my-team",
    "protocol": "HTTP",
    "filterChains": [{
      "name": "default",
      "filters": [{
        "name": "envoy.filters.network.http_connection_manager",
        "type": "httpConnectionManager",
        "routeConfigName": "compression-test-routes",
        "httpFilters": [
          {"filter": {"type": "router"}}
        ]
      }]
    }]
  }'
```

### Step 3: Install Filter on Listener

```bash
curl -X POST http://localhost:8080/api/v1/filters/{filter_id}/installations \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "listenerName": "compression-test-listener",
    "order": 1
  }'
```

### Step 4: Configure Filter at Route-Config Level

```bash
curl -X POST http://localhost:8080/api/v1/filters/{filter_id}/configurations \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "scopeType": "route-config",
    "scopeId": "compression-test-routes"
  }'
```

## Testing Compression

### Test with Accept-Encoding Header

```bash
curl -v http://localhost:10093/testing/compression \
  -H "Host: compression-test.local" \
  -H "Accept-Encoding: gzip, deflate" \
  -H "Accept: application/json"
```

Expected response headers when compression is active:
```
Content-Encoding: gzip
Vary: Accept-Encoding
```

### Test Without Accept-Encoding

```bash
curl -v http://localhost:10093/testing/compression \
  -H "Host: compression-test.local" \
  -H "Accept: application/json"
```

Response should NOT have `Content-Encoding` header (uncompressed).

### Verify Compression via Stats

```bash
curl -s "http://localhost:9902/stats?filter=compressor&format=json" | jq '.stats'
```

Key metrics to check:
- `compressor.*.header_compressor_used` - Number of compressed responses
- `compressor.*.response.compressed` - Responses that were compressed
- `compressor.*.response.not_compressed` - Responses skipped
- `compressor.*.response.total_uncompressed_bytes` - Original size
- `compressor.*.response.total_compressed_bytes` - Compressed size

### Calculate Compression Ratio

```bash
#!/bin/bash
# Fetch stats and calculate compression ratio

STATS=$(curl -s "http://localhost:9902/stats?filter=compressor&format=json")

UNCOMPRESSED=$(echo $STATS | jq '.stats[] | select(.name | contains("total_uncompressed_bytes")) | .value')
COMPRESSED=$(echo $STATS | jq '.stats[] | select(.name | contains("total_compressed_bytes")) | .value')

if [ "$UNCOMPRESSED" -gt 0 ]; then
  RATIO=$(echo "scale=1; (1 - $COMPRESSED / $UNCOMPRESSED) * 100" | bc)
  echo "Compression ratio: ${RATIO}% reduction"
  echo "Original: $UNCOMPRESSED bytes"
  echo "Compressed: $COMPRESSED bytes"
fi
```

## Complete Examples

### High Compression (Best Ratio)

For static assets where CPU is not a concern:

```json
{
  "name": "high-compression-filter",
  "filterType": "compressor",
  "config": {
    "type": "compressor",
    "config": {
      "response_direction_config": {
        "common_config": {
          "min_content_length": 256,
          "content_type": [
            "application/json",
            "text/html",
            "text/css",
            "application/javascript",
            "text/plain",
            "application/xml"
          ]
        }
      },
      "compressor_library": {
        "type": "gzip",
        "compression_level": "best_compression",
        "memory_level": 9,
        "window_bits": 15
      }
    }
  },
  "team": "my-team"
}
```

### Fast Compression (Best Speed)

For real-time APIs where latency matters:

```json
{
  "name": "fast-compression-filter",
  "filterType": "compressor",
  "config": {
    "type": "compressor",
    "config": {
      "response_direction_config": {
        "common_config": {
          "min_content_length": 1024,
          "content_type": ["application/json"]
        }
      },
      "compressor_library": {
        "type": "gzip",
        "compression_level": "best_speed",
        "memory_level": 4,
        "window_bits": 10
      }
    }
  },
  "team": "my-team"
}
```

### Brotli Compression (Better Ratios)

Brotli typically achieves 15-25% better compression than gzip:

```json
{
  "name": "brotli-compression-filter",
  "filterType": "compressor",
  "config": {
    "type": "compressor",
    "config": {
      "response_direction_config": {
        "common_config": {
          "min_content_length": 256,
          "content_type": [
            "application/json",
            "text/html",
            "text/css",
            "application/javascript"
          ]
        }
      },
      "compressor_library": {
        "type": "brotli",
        "quality": 4,
        "window_bits": 18
      }
    }
  },
  "team": "my-team"
}
```

### ETag-Aware Compression

Skip compression for cacheable responses with ETag:

```json
{
  "name": "etag-aware-compression",
  "filterType": "compressor",
  "config": {
    "type": "compressor",
    "config": {
      "response_direction_config": {
        "common_config": {
          "min_content_length": 100,
          "content_type": ["application/json", "text/html"],
          "disable_on_etag_header": true
        }
      },
      "compressor_library": {
        "type": "gzip",
        "compression_level": "default_compression"
      }
    }
  },
  "team": "my-team"
}
```

### Selective Content-Type Compression

Compress only specific API responses:

```json
{
  "name": "api-compression-filter",
  "filterType": "compressor",
  "config": {
    "type": "compressor",
    "config": {
      "response_direction_config": {
        "common_config": {
          "min_content_length": 500,
          "content_type": [
            "application/json",
            "application/vnd.api+json",
            "application/graphql+json"
          ]
        }
      },
      "compressor_library": {
        "type": "gzip",
        "compression_level": "compression_level_4"
      }
    }
  },
  "team": "my-team"
}
```

## Per-Route Configuration

### Disable Compression for a Route

For routes serving already-compressed content (images, videos):

```json
{
  "typedPerFilterConfig": {
    "envoy.filters.http.compressor": {
      "filter_type": "compressor",
      "disabled": true
    }
  }
}
```

### Override with Different Settings

Apply stronger compression for specific routes:

```json
{
  "typedPerFilterConfig": {
    "envoy.filters.http.compressor": {
      "filter_type": "compressor",
      "response_direction_config": {
        "common_config": {
          "min_content_length": 50,
          "content_type": ["application/json"]
        }
      },
      "compressor_library": {
        "type": "gzip",
        "compression_level": "best_compression"
      }
    }
  }
}
```

## Algorithm Comparison

| Algorithm | Compression Ratio | CPU Usage | Browser Support | Best For |
|-----------|------------------|-----------|-----------------|----------|
| **gzip** | Good | Low | Universal | General use, compatibility |
| **Brotli** | Better (15-25% smaller) | Medium | Modern browsers | Static assets, HTML |
| **Zstandard** | Best | Medium | Limited | Internal APIs, HTTP/2+ |

### Browser Support

- **gzip**: All browsers
- **Brotli**: Chrome 50+, Firefox 44+, Edge 15+, Safari 11+
- **Zstandard**: Experimental (Chrome with flag)

## Troubleshooting

### Common Issues

1. **No Compression Happening**

   Check prerequisites:
   - Client sends `Accept-Encoding: gzip` (or brotli, zstd)
   - Response `Content-Type` matches configured types
   - Response size >= `min_content_length`

   ```bash
   # Verify request includes Accept-Encoding
   curl -v http://localhost:10093/path \
     -H "Accept-Encoding: gzip, deflate"
   ```

2. **Response Too Small**

   If response body < `min_content_length`, compression is skipped.
   Check the default (30 bytes) or your configured value.

3. **Wrong Content-Type**

   Verify the response Content-Type matches your configured list:
   ```bash
   curl -I http://localhost:10093/path
   # Check Content-Type header
   ```

4. **ETag Blocking Compression**

   If `disable_on_etag_header: true` and response has ETag, compression is skipped.

5. **Upstream Already Compressed**

   If upstream sends `Content-Encoding: gzip`, Envoy won't double-compress.

### Debug Checklist

```bash
# 1. Check filter is installed
curl -s "http://localhost:9902/config_dump?resource=dynamic_listeners" | \
  jq '.configs[].active_state.listener.filter_chains[].filters[].typed_config.http_filters[] | select(.name | contains("compressor"))'

# 2. Check compression stats
curl -s "http://localhost:9902/stats" | grep compressor

# 3. Test with verbose output
curl -v http://localhost:10093/path \
  -H "Host: your-domain.local" \
  -H "Accept-Encoding: gzip, deflate"

# 4. Check response headers
curl -I http://localhost:10093/path \
  -H "Accept-Encoding: gzip" | grep -i "content-encoding\|vary"
```

### Metrics

| Metric | Description |
|--------|-------------|
| `compressor.{name}.header_compressor_used` | Responses where compressor was applied |
| `compressor.{name}.header_compressor_overshadowed` | Responses where another compressor took precedence |
| `compressor.{name}.response.compressed` | Responses successfully compressed |
| `compressor.{name}.response.not_compressed` | Responses not compressed (too small, wrong type, etc.) |
| `compressor.{name}.response.total_uncompressed_bytes` | Total original bytes before compression |
| `compressor.{name}.response.total_compressed_bytes` | Total bytes after compression |

## Performance Considerations

1. **CPU vs Bandwidth Trade-off**: Higher compression levels use more CPU but save bandwidth. Choose based on your constraints.

2. **Minimum Content Length**: Set `min_content_length` high enough (100-256 bytes) to avoid overhead for tiny responses.

3. **Memory Level**: Higher `memory_level` (for gzip) improves compression but uses more memory per connection.

4. **Content-Type Filtering**: Only compress compressible content. Images, videos, and already-compressed formats gain nothing.

5. **Caching Considerations**: Compressed responses with `Vary: Accept-Encoding` create separate cache entries per encoding.

## Security Considerations

1. **BREACH Attack**: Be aware that compression of HTTPS responses containing secrets can leak information. Consider disabling compression for responses containing CSRF tokens or sensitive data.

2. **Resource Exhaustion**: Very large responses can consume significant CPU during compression. Consider rate limiting or max body size limits.

3. **Decompression Bombs**: When accepting compressed request bodies, validate uncompressed size to prevent decompression bombs.

## See Also

- [Filters Overview](../filters.md) - All available filters
- [Header Mutation](./header_mutation.md) - Add custom headers to requests/responses
- [Local Rate Limit](./local_rate_limit.md) - Rate limiting per instance
