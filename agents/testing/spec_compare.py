"""Compare a learned OpenAPI spec against a reference spec."""
from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Any


@dataclass
class SpecMatchResult:
    """Result of comparing learned spec against reference."""
    reference_endpoints: set[tuple[str, str]]   # (method, path) from reference
    learned_endpoints: set[tuple[str, str]]      # (method, path) from learned
    matched: set[tuple[str, str]]                # intersection
    missing: set[tuple[str, str]]                # in reference but not learned
    extra: set[tuple[str, str]]                  # in learned but not reference
    path_coverage: float                          # % of reference paths found
    endpoint_coverage: float                      # % of reference method+path found
    schema_coverage: float                        # response body field coverage
    header_coverage: float                        # header/auth discovery coverage

    @property
    def score(self) -> float:
        """Weighted overall score."""
        return (
            self.endpoint_coverage * 0.4 +
            self.schema_coverage * 0.4 +
            self.header_coverage * 0.2
        )


def normalize_path(path: str) -> str:
    """Normalize path parameters for comparison.

    /api/articles/{slug} and /api/articles/:slug and /api/articles/{id}
    should all match. Replace any path parameter with {param}.
    """
    # Replace :param style
    path = re.sub(r':([a-zA-Z_]+)', '{param}', path)
    # Normalize {anything} to {param}
    path = re.sub(r'\{[^}]+\}', '{param}', path)
    return path


def extract_endpoints(spec: dict) -> set[tuple[str, str]]:
    """Extract (METHOD, normalized_path) pairs from an OpenAPI spec."""
    endpoints = set()
    paths = spec.get("paths", {})
    for path, methods in paths.items():
        norm = normalize_path(path)
        for method in methods:
            if method.lower() in ("get", "post", "put", "patch", "delete"):
                endpoints.add((method.upper(), norm))
    return endpoints


def resolve_refs(schema: dict, root: dict, seen: set[str] | None = None) -> dict:
    """Recursively resolve all ``$ref`` pointers in *schema* against *root*.

    Args:
        schema: A schema node (may contain ``$ref``).
        root:   The full OpenAPI spec used to look up ``#/...`` pointers.
        seen:   Tracks visited ``$ref`` strings to break cycles.

    Returns:
        A new dict with every ``$ref`` replaced by the resolved content.
    """
    if seen is None:
        seen = set()

    if not isinstance(schema, dict):
        return schema

    ref = schema.get("$ref")
    if ref is not None:
        if ref in seen:
            return {}
        seen = seen | {ref}  # copy so sibling branches don't share state
        resolved = _walk_ref(ref, root)
        if resolved is None:
            return {}
        return resolve_refs(resolved, root, seen)

    out: dict[str, Any] = {}
    for key, value in schema.items():
        if isinstance(value, dict):
            out[key] = resolve_refs(value, root, seen)
        elif isinstance(value, list):
            out[key] = [
                resolve_refs(item, root, seen) if isinstance(item, dict) else item
                for item in value
            ]
        else:
            out[key] = value
    return out


def _walk_ref(ref: str, root: dict) -> dict | None:
    """Follow a JSON-pointer ``$ref`` like ``#/components/schemas/Foo``."""
    if not ref.startswith("#/"):
        return None
    parts = ref[2:].split("/")
    node: Any = root
    for part in parts:
        if isinstance(node, dict):
            node = node.get(part)
        else:
            return None
        if node is None:
            return None
    return node if isinstance(node, dict) else None


def extract_field_names(schema: dict, prefix: str = "", root: dict | None = None) -> set[str]:
    """Recursively extract field name paths from a JSON Schema.

    e.g., {"articles": [{"title": "...", "author": {"username": "..."}}]}
    yields: {"articles", "articles[].title", "articles[].author", "articles[].author.username"}

    If *root* is provided, ``$ref`` pointers are resolved first.
    """
    if root is not None:
        schema = resolve_refs(schema, root)

    fields = set()
    if schema.get("type") == "object":
        for prop, prop_schema in schema.get("properties", {}).items():
            full = f"{prefix}.{prop}" if prefix else prop
            fields.add(full)
            # root=None because refs are already resolved at the top level
            fields |= extract_field_names(prop_schema, full)
    elif schema.get("type") == "array":
        items = schema.get("items", {})
        arr_prefix = f"{prefix}[]" if prefix else "[]"
        fields |= extract_field_names(items, arr_prefix)
    return fields


def _get_response_schema(spec: dict, method: str, path: str, root: dict | None = None) -> dict | None:
    """Extract the 200/201 response schema for a given endpoint.

    If *root* is provided, ``$ref`` pointers in the schema are resolved.
    """
    effective_root = root or spec
    path_item = spec.get("paths", {}).get(path, {})
    op = path_item.get(method.lower(), {})
    responses = op.get("responses", {})
    for code in ("200", "201"):
        resp = responses.get(code, {})
        # The response itself might be a $ref (e.g. $ref: '#/components/responses/NotFound')
        if "$ref" in resp:
            resp = resolve_refs(resp, effective_root)
        content = resp.get("content", {})
        json_content = content.get("application/json", {})
        schema = json_content.get("schema")
        if schema:
            return resolve_refs(schema, effective_root) if root is not None else schema
    return None


def compare_response_schemas(
    learned: dict,
    reference: dict,
    matched_endpoints: set[tuple[str, str]],
) -> float:
    """Compare response body field names for matched endpoints.

    Returns: fraction of reference fields found in learned spec (averaged across endpoints).
    Only compares field names and nesting structure, not descriptions or validation rules.

    Endpoints where the learned spec has no response content (bodyless observations
    like GET collections without ExtProc body capture) are excluded from scoring —
    they were correctly discovered but the response body wasn't observable.
    """
    scores = []
    for method, path in matched_endpoints:
        ref_schema = _get_response_schema(reference, method, path, root=reference)
        learned_schema = _get_response_schema(learned, method, path, root=learned)
        if not ref_schema:
            continue
        # Skip endpoints where no response body was captured — the endpoint was
        # discovered but the response wasn't observable via ExtProc.
        if not learned_schema:
            continue
        ref_fields = extract_field_names(ref_schema)
        learned_fields = extract_field_names(learned_schema)
        if ref_fields:
            scores.append(len(ref_fields & learned_fields) / len(ref_fields))
    return sum(scores) / len(scores) if scores else 0.0


def compare_headers(
    learned: dict,
    reference: dict,
    matched_endpoints: set[tuple[str, str]],
) -> float:
    """Compare request header/parameter discovery for matched endpoints.

    Checks: did the learned spec identify endpoints that require Authorization,
    Content-Type, and other significant headers?
    """
    ref_headers: set[tuple[str, str, str]] = set()
    learned_headers: set[tuple[str, str, str]] = set()

    for method, path in matched_endpoints:
        for spec, target_set in [(reference, ref_headers), (learned, learned_headers)]:
            op = spec.get("paths", {}).get(path, {}).get(method.lower(), {})
            # Check parameters with in=header (resolve $ref if present)
            for param in op.get("parameters", []):
                if "$ref" in param:
                    param = resolve_refs(param, spec)
                if param.get("in") == "header":
                    target_set.add((method, path, param["name"].lower()))
            # Check security schemes (implies Authorization header)
            if op.get("security") or op.get("securitySchemes"):
                target_set.add((method, path, "authorization"))

    if not ref_headers:
        return 1.0  # nothing to compare
    return len(ref_headers & learned_headers) / len(ref_headers)


def compare_specs(learned: dict, reference: dict) -> SpecMatchResult:
    """Compare a learned OpenAPI spec against a reference."""
    ref_endpoints = extract_endpoints(reference)
    learned_endpoints = extract_endpoints(learned)

    matched = ref_endpoints & learned_endpoints
    missing = ref_endpoints - learned_endpoints
    extra = learned_endpoints - ref_endpoints

    ref_paths = {p for _, p in ref_endpoints}
    learned_paths = {p for _, p in learned_endpoints}
    path_coverage = len(ref_paths & learned_paths) / len(ref_paths) if ref_paths else 0.0
    endpoint_coverage = len(matched) / len(ref_endpoints) if ref_endpoints else 0.0

    schema_cov = compare_response_schemas(learned, reference, matched)
    header_cov = compare_headers(learned, reference, matched)

    return SpecMatchResult(
        reference_endpoints=ref_endpoints,
        learned_endpoints=learned_endpoints,
        matched=matched,
        missing=missing,
        extra=extra,
        path_coverage=path_coverage,
        endpoint_coverage=endpoint_coverage,
        schema_coverage=schema_cov,
        header_coverage=header_cov,
    )
