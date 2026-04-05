# Frontend Reference (for backend developers)

## Tech Stack

SvelteKit 2 (Svelte 5 with runes), Tailwind CSS 3, shadcn-svelte, oidc-client-ts, Zod + sveltekit-superforms.

## Directory Layout

```
ui/src/
├── lib/
│   ├── api/client.ts      # Singleton ApiClient (60+ methods, native fetch, auto Bearer token)
│   ├── auth/oidc-config.ts # OIDC UserManager setup (Zitadel, PKCE code flow)
│   ├── stores/             # Svelte writable stores (team, org, stats, adminSummary)
│   ├── components/         # Reusable UI (~36 shared + domain subdirs)
│   ├── types/              # TypeScript interfaces
│   ├── schemas/            # Zod validation schemas
│   └── utils/              # Error handling, validators, permissions
└── routes/
    ├── login/              # OIDC sign-in
    ├── auth/callback/      # OIDC redirect handler
    ├── bootstrap/          # First-run setup
    └── (authenticated)/    # All protected routes (auth guard in layout)
        ├── dashboard/, clusters/, listeners/, routes/, route-configs/
        ├── filters/, custom-filters/, secrets/, dataplanes/
        ├── learning/, imports/, mcp-tools/, mcp-connections/
        ├── organizations/, admin/, profile/, stats/
        └── generate-envoy-config/
```

## API Client

**File:** `ui/src/lib/api/client.ts`

- Base URL from `PUBLIC_API_BASE` env var (default `http://localhost:8080`)
- Auto-injects OIDC JWT as `Authorization: Bearer <token>`
- 401 response → clears auth, redirects to `/login`
- Response validation via Zod schemas

## Auth Flow

1. User visits `/login` → clicks sign in → `userManager.signinRedirect()` to Zitadel
2. Callback at `/auth/callback` → `signinRedirectCallback()` → token in localStorage
3. `(authenticated)` layout guard checks token validity on every navigation
4. OIDC config fetched at runtime from `GET /api/v1/auth/config` (issuer, client_id)
5. Auto-renewal via `automaticSilentRenew: true`

## State Management

Lightweight Svelte stores — no Redux/Pinia:
- `selectedTeam` (sessionStorage-persisted) — drives API filtering
- `currentOrg` — org context + role for permission checks
- Component state via Svelte 5 `$state` runes
- Forms via `useFormSubmit()` hook + Zod validation

## Backend Developer Essentials

- **Add a new API endpoint?** Update `ui/src/lib/api/client.ts` with a method for it
- **Add a new resource type?** Create route dir in `routes/(authenticated)/`, add CRUD pages
- **Env config:** `PUBLIC_API_BASE` is the only backend-relevant env var
- **Auth config endpoint:** Frontend expects `GET /api/v1/auth/config` to return OIDC params
- **Team scoping:** All API calls include team context from the `selectedTeam` store
