# Frontend Integration Guide

This guide shows how to integrate Flowplane's session-based authentication into web applications.

## Quick Start

### 1. Bootstrap and Login Flow

```javascript
// Step 1: Bootstrap (only on first setup)
async function bootstrap(adminEmail) {
  const response = await fetch('/api/v1/bootstrap/initialize', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ adminEmail })
  });

  if (!response.ok) {
    throw new Error('Bootstrap failed: system may already be initialized');
  }

  const { setupToken } = await response.json();
  return setupToken;
}

// Step 2: Create session from setup token
async function createSession(setupToken) {
  const response = await fetch('/api/v1/auth/sessions', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'include', // Important: include cookies
    body: JSON.stringify({ setupToken })
  });

  if (!response.ok) {
    throw new Error('Session creation failed');
  }

  const { csrfToken, sessionId, expiresAt } = await response.json();

  // Store CSRF token in memory (NOT localStorage - XSS risk)
  sessionStorage.setItem('csrfToken', csrfToken);

  return { csrfToken, sessionId, expiresAt };
}
```

### 2. Making API Requests

```javascript
// GET requests (no CSRF required)
async function getSessionInfo() {
  const response = await fetch('/api/v1/auth/sessions/me', {
    credentials: 'include' // Include session cookie
  });

  return await response.json();
}

// POST/PUT/PATCH/DELETE requests (CSRF required)
async function createCluster(clusterData) {
  const csrfToken = sessionStorage.getItem('csrfToken');

  const response = await fetch('/api/v1/clusters', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'X-CSRF-Token': csrfToken // Required for state-changing requests
    },
    credentials: 'include',
    body: JSON.stringify(clusterData)
  });

  if (response.status === 403) {
    throw new Error('CSRF validation failed');
  }

  return await response.json();
}
```

### 3. Logout

```javascript
async function logout() {
  const csrfToken = sessionStorage.getItem('csrfToken');

  const response = await fetch('/api/v1/auth/sessions/logout', {
    method: 'POST',
    headers: {
      'X-CSRF-Token': csrfToken
    },
    credentials: 'include'
  });

  // Clear stored CSRF token
  sessionStorage.removeItem('csrfToken');

  // Redirect to login page
  window.location.href = '/login';
}
```

## React Example

### Authentication Context

```typescript
import React, { createContext, useContext, useState, useEffect } from 'react';

interface AuthContextType {
  isAuthenticated: boolean;
  csrfToken: string | null;
  sessionInfo: SessionInfo | null;
  login: (setupToken: string) => Promise<void>;
  logout: () => Promise<void>;
}

const AuthContext = createContext<AuthContextType | null>(null);

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [isAuthenticated, setIsAuthenticated] = useState(false);
  const [csrfToken, setCsrfToken] = useState<string | null>(null);
  const [sessionInfo, setSessionInfo] = useState<SessionInfo | null>(null);

  // Check if session is valid on mount
  useEffect(() => {
    checkSession();
  }, []);

  async function checkSession() {
    try {
      const response = await fetch('/api/v1/auth/sessions/me', {
        credentials: 'include'
      });

      if (response.ok) {
        const info = await response.json();
        setSessionInfo(info);
        setIsAuthenticated(true);

        // Retrieve CSRF token from storage
        const stored = sessionStorage.getItem('csrfToken');
        if (stored) {
          setCsrfToken(stored);
        }
      }
    } catch (error) {
      console.error('Session check failed:', error);
    }
  }

  async function login(setupToken: string) {
    const response = await fetch('/api/v1/auth/sessions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      credentials: 'include',
      body: JSON.stringify({ setupToken })
    });

    if (!response.ok) {
      throw new Error('Login failed');
    }

    const data = await response.json();

    setCsrfToken(data.csrfToken);
    sessionStorage.setItem('csrfToken', data.csrfToken);
    setIsAuthenticated(true);
    setSessionInfo(data);
  }

  async function logout() {
    if (csrfToken) {
      await fetch('/api/v1/auth/sessions/logout', {
        method: 'POST',
        headers: { 'X-CSRF-Token': csrfToken },
        credentials: 'include'
      });
    }

    setCsrfToken(null);
    sessionStorage.removeItem('csrfToken');
    setIsAuthenticated(false);
    setSessionInfo(null);
  }

  return (
    <AuthContext.Provider value={{ isAuthenticated, csrfToken, sessionInfo, login, logout }}>
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth() {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error('useAuth must be used within AuthProvider');
  }
  return context;
}
```

### API Client Hook

```typescript
import { useAuth } from './AuthContext';

export function useApi() {
  const { csrfToken, logout } = useAuth();

  async function request(url: string, options: RequestInit = {}) {
    const headers: HeadersInit = {
      ...options.headers,
      'Content-Type': 'application/json'
    };

    // Add CSRF token for state-changing requests
    if (['POST', 'PUT', 'PATCH', 'DELETE'].includes(options.method || 'GET')) {
      if (!csrfToken) {
        throw new Error('No CSRF token available');
      }
      headers['X-CSRF-Token'] = csrfToken;
    }

    const response = await fetch(url, {
      ...options,
      headers,
      credentials: 'include'
    });

    // Handle authentication errors
    if (response.status === 401) {
      await logout();
      throw new Error('Session expired');
    }

    if (response.status === 403) {
      throw new Error('Forbidden: Invalid CSRF token');
    }

    if (!response.ok) {
      throw new Error(`API error: ${response.statusText}`);
    }

    return response.json();
  }

  return { request };
}
```

### Usage in Components

```typescript
import { useApi } from './useApi';

export function ClusterList() {
  const { request } = useApi();
  const [clusters, setClusters] = useState([]);

  async function loadClusters() {
    const data = await request('/api/v1/clusters');
    setClusters(data);
  }

  async function createCluster(name: string, endpoints: any[]) {
    await request('/api/v1/clusters', {
      method: 'POST',
      body: JSON.stringify({ name, endpoints })
    });
    await loadClusters();
  }

  return (
    <div>
      {/* UI components */}
    </div>
  );
}
```

## Vue.js Example

### Auth Plugin

```javascript
// plugins/auth.js
export default {
  install(app) {
    const auth = {
      csrfToken: null,
      isAuthenticated: false,

      async login(setupToken) {
        const response = await fetch('/api/v1/auth/sessions', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          credentials: 'include',
          body: JSON.stringify({ setupToken })
        });

        const data = await response.json();
        this.csrfToken = data.csrfToken;
        this.isAuthenticated = true;
        sessionStorage.setItem('csrfToken', data.csrfToken);
      },

      async logout() {
        if (this.csrfToken) {
          await fetch('/api/v1/auth/sessions/logout', {
            method: 'POST',
            headers: { 'X-CSRF-Token': this.csrfToken },
            credentials: 'include'
          });
        }
        this.csrfToken = null;
        this.isAuthenticated = false;
        sessionStorage.removeItem('csrfToken');
      },

      getHeaders(method = 'GET') {
        const headers = { 'Content-Type': 'application/json' };
        if (['POST', 'PUT', 'PATCH', 'DELETE'].includes(method)) {
          headers['X-CSRF-Token'] = this.csrfToken;
        }
        return headers;
      }
    };

    // Initialize CSRF token from storage
    const stored = sessionStorage.getItem('csrfToken');
    if (stored) {
      auth.csrfToken = stored;
      auth.isAuthenticated = true;
    }

    app.config.globalProperties.$auth = auth;
    app.provide('auth', auth);
  }
};
```

## Common Patterns

### Axios Interceptor

```javascript
import axios from 'axios';

// Create axios instance with defaults
const api = axios.create({
  baseURL: '/api/v1',
  withCredentials: true // Include cookies
});

// Request interceptor: add CSRF token
api.interceptors.request.use((config) => {
  const csrfToken = sessionStorage.getItem('csrfToken');

  if (['post', 'put', 'patch', 'delete'].includes(config.method)) {
    if (csrfToken) {
      config.headers['X-CSRF-Token'] = csrfToken;
    }
  }

  return config;
});

// Response interceptor: handle auth errors
api.interceptors.response.use(
  (response) => response,
  (error) => {
    if (error.response?.status === 401) {
      // Session expired - redirect to login
      window.location.href = '/login';
    }
    return Promise.reject(error);
  }
);

export default api;
```

### Fetch Wrapper

```javascript
class ApiClient {
  constructor() {
    this.baseUrl = '/api/v1';
  }

  async request(endpoint, options = {}) {
    const url = `${this.baseUrl}${endpoint}`;
    const headers = {
      'Content-Type': 'application/json',
      ...options.headers
    };

    // Add CSRF token for state-changing requests
    if (['POST', 'PUT', 'PATCH', 'DELETE'].includes(options.method)) {
      const csrfToken = sessionStorage.getItem('csrfToken');
      if (csrfToken) {
        headers['X-CSRF-Token'] = csrfToken;
      }
    }

    const response = await fetch(url, {
      ...options,
      headers,
      credentials: 'include'
    });

    if (response.status === 401) {
      // Clear auth state and redirect
      sessionStorage.removeItem('csrfToken');
      window.location.href = '/login';
      throw new Error('Unauthorized');
    }

    if (!response.ok) {
      throw new Error(`API error: ${response.status}`);
    }

    return response.json();
  }

  get(endpoint) {
    return this.request(endpoint, { method: 'GET' });
  }

  post(endpoint, data) {
    return this.request(endpoint, {
      method: 'POST',
      body: JSON.stringify(data)
    });
  }

  // ... similar methods for PUT, PATCH, DELETE
}

export const api = new ApiClient();
```

## Security Checklist

- ✅ Always use `credentials: 'include'` to send cookies
- ✅ Store CSRF tokens in `sessionStorage`, never `localStorage`
- ✅ Include CSRF token in `X-CSRF-Token` header for POST/PUT/PATCH/DELETE
- ✅ Handle 401 errors by redirecting to login
- ✅ Handle 403 errors as CSRF validation failures
- ✅ Use HTTPS in production
- ✅ Implement proper logout that calls the API endpoint
- ✅ Clear CSRF token on logout
- ✅ Never log or expose CSRF tokens in console/errors

## Testing

### Mock API for Development

```javascript
// Mock session creation for testing
if (process.env.NODE_ENV === 'development') {
  window.mockLogin = async () => {
    return {
      csrfToken: 'mock-csrf-token',
      sessionId: 'mock-session-id',
      expiresAt: new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString(),
      teams: ['test-team'],
      scopes: ['admin:*']
    };
  };
}
```

## Troubleshooting

### CORS Issues

Ensure your API server has proper CORS configuration:

```javascript
// Server-side CORS configuration
app.use(cors({
  origin: 'http://localhost:3000', // Your frontend URL
  credentials: true // Allow cookies
}));
```

### Cookie Not Being Set

- Verify `credentials: 'include'` is set in fetch options
- Check that API and frontend are on the same domain or CORS is properly configured
- Ensure cookies are not blocked by browser settings

### CSRF Token Missing

- Verify token is being stored after session creation
- Check that token is being retrieved and included in headers
- Ensure `sessionStorage` is not being cleared unexpectedly

## See Also

- [Session Management](./session-management.md) - Complete session management documentation
- [Security Best Practices](./security-best-practices.md) - Security guidelines
- [API Reference](./api.md) - Full API documentation
