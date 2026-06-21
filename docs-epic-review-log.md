
## Docs taxonomy policy (post-#100)
- Drafted `docs/README.md` + `internal/README.md` (user vs internal split, Diátaxis mode dirs, concepts/ for explanation). No file moves.
- Tracking issue #116; comment on epic #100.
- Self-review of READMEs found: (🔴) absolute "no docs→internal/spec links" CI rule would reject docs/README.md itself; (🟠) internal table conflated current vs planned paths. Fixed: rule now carves out the index file + bucket-index README links; internal table tags rows (current)/(planned — #116). Synced wording across both READMEs + #116.

## Phase 2 — per-commit Codex gate (branch docs/flowplane-v2)
Branch docs/flowplane-v2 created off docs/epic-100 (includes taxonomy policy READMEs the docs depend on).

### #111 Reference: error codes — COMMITTED 82560ff (3 passes)
- pass1 CHANGES: claimed details logged on internal errors; code logs only request_id+message. Fixed.
- pass2 CHANGES: details typed as "object"; actually Option<serde_json::Value> (any JSON). Fixed.
- pass3 APPROVED. Also applied non-blocking audience fix (api-consumers, operators).

### #107 Reference: configuration & env vars — BLOCKED (3 passes, not committed)
- pass1 CHANGES: dup rows (CONFIG/OIDC_ISSUER), missing Component col, CLI default col, "pure tables" mode.
- pass2 CHANGES: missing CLI defaults — config path ~/.flowplane/config.toml, scope "openid email profile", callback http://127.0.0.1:8976/callback. (verified + fixed)
- pass3 CHANGES (cap hit): new blockers — agent clamps (poll>=1, queue 1..=16384, loopback-only plaintext), secret-key format constraints, re-raised "pure tables" mode objection. Var list confirmed exhaustive/accurate (non-blocking).
- Action: git restore --staged; label blocked; commented with outstanding items + recommendation (clarify "pure tables" AC; items 1-2 are easy accuracy follow-ups). Draft kept at docs/reference/configuration.md (untracked). Moving on.

### #109 Reference: CLI — COMMITTED 1688e74 (1 pass, APPROVED)

### #110 Reference: HTTP filter catalogue — COMMITTED 4e286ca (2 passes)
- pass1 CHANGES: claimed global_rate_limit disablable per-scope; domain accepts but xDS envoy_filter_name maps only 8 kinds → translation fails. Fixed with caveat (7 effectively-disablable kinds).
- pass2 APPROVED.

### #108 Reference: REST API — COMMITTED f26cbb6 (2 passes)
- pass1 CHANGES: If-Match also on secrets rotate POST (not just PATCH/DELETE); not all list endpoints use ListQuery/Page. Also fixed missing-If-Match status 422->400. Fixed.
- pass2 APPROVED (2 non-blocking nits: learning/discovery list query also has status; drift-claim scope — left, defers to OpenAPI).

### #106 How-to: CLI auth & contexts — COMMITTED 6590e88 (1 pass, APPROVED)

### #101 Tutorial: getting started — COMMITTED 5f9a20c (2 passes)
- pass1 CHANGES: stopped at resource-list (AC needs curl-through-Envoy); dev-oidc wrongly called debug-only (it is a DEFAULT feature; release container --no-default-features rejects dev mode). Fixed: added dataplane create+bootstrap+envoy+curl; corrected build gating.
- pass2 APPROVED.

### #112 Explanation: concepts — COMMITTED 500edb0 (3 passes)
- pass1 CHANGES: org "never inferred" wrong (single membership inferred); overstated repos take TeamScope. Fixed.
- pass2 CHANGES: restart priming != serves same bytes; rebuilds from DB, quarantine not restored. Fixed.
- pass3 APPROVED.

### #103 How-to: dataplane + mTLS cert — COMMITTED d45e53d (2 passes)
- pass1 CHANGES: issued ca_certificate_pem wrongly mapped to agent --tls-ca-path (that is the CP server-CA, not the client-chain issuer CA). Fixed mapping.
- pass2 APPROVED.

### #104 How-to: AI gateway route + budget — COMMITTED bb85e09 (2 passes)
- pass1 CHANGES: prompt_token_weight default is 0 not 1; enforcing update missing --revision/If-Match. Fixed.
- pass2 APPROVED.

### #102 How-to: JWT auth + rate limit — COMMITTED b62a4fa (2 passes)
- pass1 CHANGES: jwt scoping via empty rules misframed; local_rate_limit chain entry called optional (must be in chain); bad tutorial link; wrong Retry-After claim on dataplane 429. Fixed (empty-rules concern was a code-comment artifact; emitted proto leaves rules empty — verified).
- pass2 APPROVED.

### #105 How-to: learning/discovery -> publish spec — COMMITTED 33c2b5c (2 passes)
- pass1 CHANGES: publish SpecReviewBody body is required (only reason field optional); send {} or {reason}. Fixed.
- pass2 APPROVED.

### #107 Reference: configuration & env vars — COMMITTED 2b7feb2 (re-gated, UNBLOCKED)
- Applied Bucket A: agent clamps (poll>=1, queue 1..=16384, cp plaintext loopback-only), secret-key formats (KEY 32 bytes, KEY_ID 1..=128 no-control, KEYS JSON obj->32-byte/base64). Restructured to tables (precedence table, constraints table, TOML code block) to satisfy "pure tables".
- re-pass A CHANGES: "validated at startup" too broad — secret-key constraints are use-time, agent at agent startup. Fixed with per-row timing note.
- re-pass B APPROVED. All 12/12 sub-issues now committed; epic #100 fully checked.
