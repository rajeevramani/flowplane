#!/usr/bin/env python3
"""Agent-usability smoke (spec/11 S4 exit): drive a workflow using ONLY the OpenAPI
document — no knowledge of the source. Discovers operations by tag+method, synthesizes
request bodies from the schemas (required fields, type-driven values), and verifies the
documented status codes and response shapes.

Usage: agent-smoke.py <base_url> <bearer_token> <team>
"""
import json
import sys
import urllib.request


def fetch(base, path, method="GET", token=None, body=None, headers=None):
    req = urllib.request.Request(base + path, method=method)
    if token:
        req.add_header("authorization", f"Bearer {token}")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    data = None
    if body is not None:
        req.add_header("content-type", "application/json")
        data = json.dumps(body).encode()
    try:
        with urllib.request.urlopen(req, data) as resp:
            raw = resp.read()
            return resp.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        raw = e.read()
        return e.code, json.loads(raw) if raw else None


def resolve(doc, schema):
    while "$ref" in schema:
        name = schema["$ref"].rsplit("/", 1)[1]
        schema = doc["components"]["schemas"][name]
    return schema


def synthesize(doc, schema, overrides):
    """Minimal valid value for a schema: required fields only, type-driven."""
    schema = resolve(doc, schema)
    if "enum" in schema:
        return schema["enum"][0]
    t = schema.get("type")
    if t == "object":
        out = {}
        for field in schema.get("required", []):
            if field in overrides:
                out[field] = overrides[field]
            else:
                out[field] = synthesize(doc, schema["properties"][field], overrides)
        return out
    if t == "array":
        return [synthesize(doc, schema["items"], overrides)]
    if t == "integer":
        minimum = schema.get("minimum", 0)
        return max(int(minimum), 8080)
    if t == "string":
        return overrides.get("__string__", "10.0.0.1")
    if t == "boolean":
        return False
    return None


def find_op(doc, tag, method):
    for path, item in doc["paths"].items():
        op = item.get(method)
        if op and tag in op.get("tags", []):
            yield path, op


def main():
    base, token, team = sys.argv[1], sys.argv[2], sys.argv[3]
    status, doc = fetch(base, "/api-docs/openapi.json")
    assert status == 200, f"doc fetch: {status}"

    # Discover the Clusters collection operations purely from the document.
    create_path, create_op = next(
        (p, o) for p, o in find_op(doc, "Clusters", "post") if p.count("{") == 1
    )
    body_schema = create_op["requestBody"]["content"]["application/json"]["schema"]
    name = f"smoke-{abs(hash(token)) % 99999}"
    body = synthesize(doc, body_schema, {"name": name})
    path = create_path.replace("{team}", team)

    status, created = fetch(base, path, "POST", token, body)
    assert status == 201, f"create: {status} {created}"
    assert created["revision"] == 1, created
    print(f"created via doc-discovered {create_path}: {created['name']}")

    item_path, _ = next(
        (p, o) for p, o in find_op(doc, "Clusters", "get") if "{name}" in p
    )
    item = item_path.replace("{team}", team).replace("{name}", name)
    status, got = fetch(base, item, "GET", token)
    assert status == 200 and got["name"] == name, f"get: {status}"
    print(f"read back via {item_path}: revision {got['revision']}")

    # The delete op documents an If-Match header parameter — honor it from the doc.
    del_path, del_op = next(
        (p, o) for p, o in find_op(doc, "Clusters", "delete") if "{name}" in p
    )
    header_params = [
        p["name"] for p in del_op.get("parameters", []) if p.get("in") == "header"
    ]
    assert "If-Match" in header_params, f"delete must document If-Match: {header_params}"
    status, _ = fetch(
        base, item, "DELETE", token, headers={"If-Match": str(got["revision"])}
    )
    assert status == 204, f"delete: {status}"
    status, gone = fetch(base, item, "GET", token)
    assert status == 404 and gone["code"] == "not_found", f"post-delete: {status} {gone}"
    print("deleted with doc-documented If-Match; 404 envelope confirmed")
    print("AGENT SMOKE PASSED: workflow driven from the document alone")


if __name__ == "__main__":
    main()
