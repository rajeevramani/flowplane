# Contributing

We appreciate bug reports, feature requests, and pull requests. Please follow the guidelines below to keep the project consistent and easy to review.

## Development Workflow
1. Fork or clone the repository.
2. Create a feature branch for your work.
3. Run the full test suite before submitting a PR:
   ```bash
   cargo fmt
   cargo clippy -- -D warnings
   cargo test
   ```
4. Update or add documentation when you introduce new features or configuration fields.
5. Submit a pull request with a clear description of the change and testing performed.

## Coding Standards
* Use Rustic naming conventions and data structures; the project relies on `serde` for JSON and `envoy-types` for protobuf definitions.
* Prefer structured config models over raw base64 payloads. If you must use `TypedConfig`, explain why in the PR.
* Keep validation close to the API boundary so errors surface early.
* Add unit tests for new translation logic. Tests that serialize/deserialize `Any` payloads help prevent regressions.

## Documentation
* Extend the appropriate document under `docs/` when adding filters, APIs, or workflows.
* Provide minimal runnable examples where possible (JSON snippets, curl commands, scripts).

## Issue Reporting
When filing issues, include:
* Description of the problem or enhancement request.
* Steps to reproduce (if a bug), including sample configuration payloads.
* Expected vs. actual behavior.

## Licensing & CLA
The project is being prepared for open-source release. Please ensure contributions can be released under the project’s license (to be finalized) and avoid submitting proprietary or confidential material.
