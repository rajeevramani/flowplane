# CORS HTTP Filter Quickstart

This guide demonstrates how to configure and use the CORS (Cross-Origin Resource Sharing) HTTP filter in Flowplane.

## Prerequisites

- Flowplane control plane running
- At least one listener configured
- Basic understanding of CORS concepts

## Basic CORS Configuration

### Step 1: Configure Listener-Level CORS Policy

Configure a basic CORS policy that allows requests from a specific domain:

```bash
curl -X PUT http://localhost:8080/listeners/api-gateway/filters/cors \
  -H "Content-Type: application/json" \
  -d '{
    "allow_origins": [
      {
        "match_type": "exact",
        "pattern": "https://app.example.com",
        "case_sensitive": true
      }
    ],
    "allow_methods": ["GET", "POST", "PUT", "DELETE"],
    "allow_headers": ["content-type", "authorization"],
    "expose_headers": ["x-request-id"],
    "allow_credentials": false,
    "max_age": 86400
  }'
```

**Expected Response:**
```json
{
  "message": "CORS filter configured successfully",
  "filter_name": "envoy.filters.http.cors",
  "config_checksum": "sha256:abc123..."
}
```

### Step 2: Verify Configuration

Retrieve the current CORS configuration:

```bash
curl -X GET http://localhost:8080/listeners/api-gateway/filters/cors
```

**Expected Response:**
```json
{
  "allow_origins": [
    {
      "match_type": "exact",
      "pattern": "https://app.example.com",
      "case_sensitive": true
    }
  ],
  "allow_methods": ["GET", "POST", "PUT", "DELETE"],
  "allow_headers": ["content-type", "authorization"],
  "expose_headers": ["x-request-id"],
  "allow_credentials": false,
  "max_age": 86400
}
```

### Step 3: Test CORS in Browser

Create a simple HTML file to test the CORS configuration:

```html
<!DOCTYPE html>
<html>
<head>
    <title>CORS Test</title>
</head>
<body>
    <h1>CORS Test</h1>
    <button onclick="testCors()">Test CORS Request</button>
    <div id="result"></div>

    <script>
        async function testCors() {
            const resultDiv = document.getElementById('result');
            try {
                const response = await fetch('http://your-api-gateway:8080/api/health', {
                    method: 'GET',
                    headers: {
                        'Content-Type': 'application/json'
                    }
                });

                if (response.ok) {
                    resultDiv.innerHTML = '<p style="color: green;">CORS request successful!</p>';
                } else {
                    resultDiv.innerHTML = '<p style="color: red;">Request failed: ' + response.status + '</p>';
                }
            } catch (error) {
                resultDiv.innerHTML = '<p style="color: red;">CORS error: ' + error.message + '</p>';
            }
        }
    </script>
</body>
</html>
```

Serve this HTML from `https://app.example.com` and click the test button.

## Advanced CORS Configurations

### Subdomain Wildcard Pattern

Allow requests from any subdomain of example.com using regex patterns:

```bash
curl -X PUT http://localhost:8080/listeners/api-gateway/filters/cors \
  -H "Content-Type: application/json" \
  -d '{
    "allow_origins": [
      {
        "match_type": "safe_regex",
        "pattern": "https://[a-z0-9-]+\\.example\\.com",
        "case_sensitive": true
      }
    ],
    "allow_methods": ["GET", "POST"],
    "allow_headers": ["content-type"],
    "allow_credentials": false,
    "max_age": 3600
  }'
```

### Multiple Origin Patterns

Configure multiple allowed origins with different patterns:

```bash
curl -X PUT http://localhost:8080/listeners/api-gateway/filters/cors \
  -H "Content-Type: application/json" \
  -d '{
    "allow_origins": [
      {
        "match_type": "exact",
        "pattern": "https://app.example.com",
        "case_sensitive": true
      },
      {
        "match_type": "exact",
        "pattern": "https://mobile.example.com",
        "case_sensitive": true
      },
      {
        "match_type": "prefix",
        "pattern": "https://dev-",
        "case_sensitive": true
      }
    ],
    "allow_methods": ["GET", "POST", "PUT", "DELETE", "PATCH"],
    "allow_headers": ["content-type", "authorization", "x-api-key"],
    "expose_headers": ["x-request-id", "x-rate-limit-remaining"],
    "allow_credentials": false,
    "max_age": 86400
  }'
```

### Credentialed Requests

Enable CORS with credential support for authenticated APIs:

```bash
curl -X PUT http://localhost:8080/listeners/secure-api/filters/cors \
  -H "Content-Type: application/json" \
  -d '{
    "allow_origins": [
      {
        "match_type": "exact",
        "pattern": "https://admin.example.com",
        "case_sensitive": true
      }
    ],
    "allow_methods": ["GET", "POST", "PUT", "DELETE"],
    "allow_headers": ["content-type", "authorization", "x-csrf-token"],
    "expose_headers": ["x-request-id"],
    "allow_credentials": true,
    "max_age": 600
  }'
```

**Important:** When `allow_credentials: true`, wildcard origins (`*`) are not allowed for security reasons.

## Per-Route CORS Configuration

### Route-Specific Override

Configure different CORS policies for specific routes:

```bash
curl -X PUT http://localhost:8080/routes/admin-api/filters/cors \
  -H "Content-Type: application/json" \
  -d '{
    "enabled": true,
    "inherit_from_listener": false,
    "cors_policy": {
      "allow_origins": [
        {
          "match_type": "exact",
          "pattern": "https://admin.example.com",
          "case_sensitive": true
        }
      ],
      "allow_methods": ["GET", "POST", "DELETE"],
      "allow_headers": ["content-type", "authorization", "x-admin-token"],
      "allow_credentials": true,
      "max_age": 300
    }
  }'
```

### Disable CORS for Specific Route

Disable CORS entirely for a specific route:

```bash
curl -X PUT http://localhost:8080/routes/internal-api/filters/cors \
  -H "Content-Type: application/json" \
  -d '{
    "enabled": false,
    "inherit_from_listener": false
  }'
```

### Inherit with Modifications

Use the listener policy but inherit from listener settings:

```bash
curl -X PUT http://localhost:8080/routes/public-api/filters/cors \
  -H "Content-Type: application/json" \
  -d '{
    "enabled": true,
    "inherit_from_listener": true
  }'
```

## Validation and Error Handling

### Test Invalid Configuration

Try to set an invalid configuration to see validation errors:

```bash
curl -X PUT http://localhost:8080/listeners/api-gateway/filters/cors \
  -H "Content-Type: application/json" \
  -d '{
    "allow_origins": [],
    "allow_methods": ["GET"],
    "max_age": 999999999999
  }'
```

**Expected Error Response:**
```json
{
  "error": "Configuration validation failed",
  "validation_errors": [
    {
      "field": "allow_origins",
      "message": "Cannot be empty",
      "code": "REQUIRED_FIELD_EMPTY"
    },
    {
      "field": "max_age",
      "message": "Exceeds maximum allowed value",
      "code": "VALUE_OUT_OF_RANGE"
    }
  ]
}
```

### Test Security Violation

Try to configure wildcard origins with credentials:

```bash
curl -X PUT http://localhost:8080/listeners/api-gateway/filters/cors \
  -H "Content-Type: application/json" \
  -d '{
    "allow_origins": [
      {
        "match_type": "exact",
        "pattern": "*",
        "case_sensitive": true
      }
    ],
    "allow_methods": ["GET"],
    "allow_credentials": true
  }'
```

**Expected Error Response:**
```json
{
  "error": "Configuration validation failed",
  "validation_errors": [
    {
      "field": "allow_credentials",
      "message": "Cannot use wildcard origins with credentials enabled",
      "code": "SECURITY_VIOLATION"
    }
  ]
}
```

## Testing CORS Behavior

### Preflight Request Test

Test OPTIONS preflight request handling:

```bash
curl -X OPTIONS http://your-api-gateway:8080/api/users \
  -H "Origin: https://app.example.com" \
  -H "Access-Control-Request-Method: POST" \
  -H "Access-Control-Request-Headers: content-type,authorization" \
  -v
```

**Expected Response Headers:**
```
Access-Control-Allow-Origin: https://app.example.com
Access-Control-Allow-Methods: GET, POST, PUT, DELETE
Access-Control-Allow-Headers: content-type, authorization
Access-Control-Max-Age: 86400
```

### Actual Request Test

Test actual cross-origin request:

```bash
curl -X POST http://your-api-gateway:8080/api/users \
  -H "Origin: https://app.example.com" \
  -H "Content-Type: application/json" \
  -d '{"name": "Test User"}' \
  -v
```

**Expected Response Headers:**
```
Access-Control-Allow-Origin: https://app.example.com
Access-Control-Expose-Headers: x-request-id
```

## Configuration Management

### Remove CORS Filter

Remove CORS filter from a listener:

```bash
curl -X DELETE http://localhost:8080/listeners/api-gateway/filters/cors
```

### Remove Route Override

Remove route-specific CORS configuration:

```bash
curl -X DELETE http://localhost:8080/routes/admin-api/filters/cors
```

## Common Patterns

### Development Environment

For development with multiple local origins:

```json
{
  "allow_origins": [
    {
      "match_type": "prefix",
      "pattern": "http://localhost:",
      "case_sensitive": true
    },
    {
      "match_type": "prefix",
      "pattern": "http://127.0.0.1:",
      "case_sensitive": true
    }
  ],
  "allow_methods": ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
  "allow_headers": ["content-type", "authorization"],
  "allow_credentials": false,
  "max_age": 86400
}
```

### Production Environment

For production with specific domains:

```json
{
  "allow_origins": [
    {
      "match_type": "exact",
      "pattern": "https://app.example.com",
      "case_sensitive": true
    },
    {
      "match_type": "exact",
      "pattern": "https://www.example.com",
      "case_sensitive": true
    }
  ],
  "allow_methods": ["GET", "POST", "PUT", "DELETE"],
  "allow_headers": ["content-type", "authorization"],
  "expose_headers": ["x-request-id"],
  "allow_credentials": true,
  "max_age": 3600
}
```

## Troubleshooting

### Common Issues

1. **CORS error in browser**: Check origin matches exactly (including protocol and port)
2. **Preflight failure**: Ensure method and headers are included in allow lists
3. **Credentials not working**: Verify no wildcard origins when credentials enabled
4. **Configuration rejected**: Check validation errors in API response

### Debug Tips

1. Use browser developer tools to inspect CORS headers
2. Test with curl to isolate client-side issues
3. Check Envoy access logs for CORS filter decisions
4. Verify filter order in listener configuration

## Next Steps

- Explore [header mutation filter](../header-mutation/quickstart.md) for dynamic header manipulation
- Learn about [JWT authentication integration](../jwt-auth/quickstart.md)
- Review [security best practices](../security/cors-security.md) for production deployment