---
name: flowplane-skill-maintainer
description: Keep Flowplane skills up to date when the codebase changes. Use when updating skills, maintaining skills, checking for skill drift, syncing skills with code, skills are outdated, skill maintenance, or after adding features to Flowplane.
license: Apache-2.0
metadata:
  author: rajeevramani
  version: "0.1.0"
---

# Flowplane Skill Maintainer

Prevents skill drift by mapping source changes to affected skills. Run the update checklist after any feature work.

## 1. Skill Inventory

| Skill | Location | Covers |
|---|---|---|
| `flowplane-dev` | `.claude/skills/flowplane-dev/` | Architecture, domain model, auth modes, boot, module map, filter system |
| `flowplane-cli` | `.claude/skills/flowplane-cli/` | CLI command reference (flags, syntax, examples) |
| `flowplane-ops` | `.claude/skills/flowplane-ops/` | Boot recipes, diagnostics (MCP + CLI), troubleshooting |
| `flowplane-api` | `.claude/skills/flowplane-api/` | Gateway config via MCP tools + CLI, learning, filters |
| `flowplane-testing` | `.claude/skills/flowplane-testing/` | Test layers, anti-patterns, E2E framework, run commands |

## 2. Source-to-Skill Mapping

When these files change, update the corresponding skills:

| Source Change | Skills to Update |
|---|---|
| `src/cli/*.rs` — new/changed CLI command | `flowplane-cli` (command reference), possibly `flowplane-api` (CLI equivalents table) |
| `src/xds/filters/http/` — new filter type | `flowplane-dev` (filter-types.md), `flowplane-api` (filter table) |
| `src/domain/filter.rs` — FilterType enum | `flowplane-dev` (filter-types.md, filter count) |
| `filter-schemas/built-in/` — new YAML schema | `flowplane-dev` (filter-types.md) |
| `src/mcp/tools/` — new MCP tool | `flowplane-api` (mcp-tools.md), possibly `flowplane-ops` (if diagnostic tool) |
| `src/mcp/tool_registry.rs` — tool authorization | `flowplane-api` (mcp-tools.md) |
| `src/auth/` — auth changes | `flowplane-dev` (auth-internals.md) |
| `src/auth/middleware.rs` — middleware changes | `flowplane-dev` (auth-internals.md), `flowplane-ops` (boot section) |
| `Makefile` — boot target changes | `flowplane-ops` (boot refs), `flowplane-dev` (boot section) |
| `docker-compose*.yml` — service changes | `flowplane-ops` (boot refs), `flowplane-dev` (boot section) |
| `CLAUDE.md` — test rule changes | `flowplane-testing` |
| `tests/e2e/` — new E2E test patterns | `flowplane-testing` (directory structure) |
| `src/config/mod.rs` — env var changes | `flowplane-dev` (boot section) |
| `src/startup.rs` — seeding changes | `flowplane-dev` (boot-modes.md) |
| `src/internal_api/` — new operations | `flowplane-dev` (module-map.md) |

## 3. Post-Feature Update Checklist

Run after any feature work:

### CLI changes
- [ ] Run `flowplane --help` and compare against `flowplane-cli` SKILL.md
- [ ] Check if new subcommands or flags need documenting
- [ ] Update `flowplane-api` Section 12 CLI equivalents table if new resource commands

### Filter changes
- [ ] Check if new filter types exist not in `flowplane-dev/references/filter-types.md`
- [ ] Verify filter count in `flowplane-dev` SKILL.md matches `FilterType` enum
- [ ] Check `filter-schemas/built-in/` for new YAML schemas
- [ ] Update `flowplane-api` Section 9 filter table

### MCP tool changes
- [ ] Check `src/mcp/tools/` for new tool modules
- [ ] Update `flowplane-api/references/mcp-tools.md`
- [ ] If diagnostic tool: update `flowplane-ops` Section 2

### Auth/boot changes
- [ ] Verify boot instructions still work for both dev and prod modes
- [ ] Check `flowplane-dev/references/boot-modes.md` and `auth-internals.md`
- [ ] Check `flowplane-ops/references/boot-dev.md` and `boot-prod.md`

### Test rule changes
- [ ] If CLAUDE.md testing sections changed, sync to `flowplane-testing`
- [ ] Check for new E2E test patterns in `tests/e2e/`

## 4. How to Update a Skill

1. **Read the current skill** — understand what it claims
2. **Read the source** — verify claims against actual code
3. **Edit the SKILL.md** — update inaccurate or missing content
4. **Edit references/** — update detailed reference docs
5. **Test the change** — spawn a teammate with a relevant prompt to validate
6. **Review output** — check the teammate produced accurate results with the updated skill

### Quick validation pattern
```
Spawn a teammate with the skill and a task that exercises the changed area.
If the teammate produces incorrect output, the skill has a gap — fix and re-test.
```

## 5. Drift Detection Quick Check

Run these commands to quickly check for drift:

```bash
# CLI commands not in skill
flowplane --help 2>/dev/null | diff - <(grep -o 'flowplane [a-z]*' .claude/skills/flowplane-cli/SKILL.md | sort -u)

# Filter types in code vs skill
grep -c 'FilterType::' src/domain/filter.rs
grep -c 'filter_type' .claude/skills/flowplane-dev/references/filter-types.md

# MCP tool count
grep -c 'pub async fn' src/mcp/tools/*.rs
grep -c 'cp_\|ops_\|devops_' .claude/skills/flowplane-api/references/mcp-tools.md
```
