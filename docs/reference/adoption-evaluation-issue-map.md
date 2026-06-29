# Adoption Evaluation Issue Map

> Audience: operators, platform-engineers, api-teams · Status: stable

This reference maps the 2.1.1 adoption/evaluation documentation issues to the public docs that close or carry them. It is a release-oriented index, not a task guide.

| Issue | Treatment | Public docs and examples |
| --- | --- | --- |
| #199 | Replaced the stale top-level `cert issue` path with the nested dataplane certificate command. | [AWS secure deployment](../how-to/aws-secure-deployment.md), `deploy/aws/README.md` |
| #200 | Repaired the no-clone quick start release selection and evaluator artifact path. | [README Quick Start](../../README.md#quick-start-no-clone-no-rust-toolchain), [Evaluate without cloning](../tutorials/evaluate-no-clone.md) |
| #201 | Documented the first-boot bootstrap token requirement for non-dev control planes. | [Production Readiness](../how-to/production-readiness.md), [Bootstrap the first platform admin](../how-to/bootstrap-platform.md) |
| #202 | Refreshed production readiness around current config, Rate Limit Service (`flowplane-rls`), and public operator links. | [Production Readiness](../how-to/production-readiness.md), [Configuration reference](configuration.md) |
| #203 | Corrected prerequisite wording where task docs referenced mismatched starter resources. | [Enable global rate limiting](../how-to/global-rate-limit.md), [Getting Started](../tutorials/getting-started.md) |
| #204 | Removed maintainer-local `internal/.env.prod-local` sourcing from evaluator deployment steps. | [AWS secure deployment](../how-to/aws-secure-deployment.md), `deploy/aws/README.md` |
| #205 | Documented token precedence consistently, including `--token` and the saved credentials fallback. | [CLI reference](cli.md), [CLI auth and contexts](../how-to/cli-auth-and-contexts.md), [Script the CLI](../how-to/script-the-cli.md) |
| #206 | Added the required dataplane/listener prerequisite before AI gateway request verification. | [AI gateway route and budget](../how-to/ai-gateway-route-budget.md), [Register dataplane mTLS](../how-to/register-dataplane-mtls.md) |
| #207 | Added provider-neutral OIDC setup guidance. | [Configure an OIDC provider](../how-to/configure-oidc-provider.md) |
| #208 | Documented how to use the immutable OIDC `sub` as `admin_subject`. | [Configure an OIDC provider](../how-to/configure-oidc-provider.md), [Bootstrap the first platform admin](../how-to/bootstrap-platform.md) |
| #209 | Added public request-body examples for clusters, listeners, and route configs. | [REST API reference](rest-api.md#gateway-resource-request-bodies) |
| #210 | Added a production-shaped control-plane/data-plane evaluation spine. | [Evaluate a production-shaped platform setup](../how-to/evaluate-platform.md) |
| #211 | Added the user, team, and grant lifecycle runbook. | [Manage users, teams, and grants](../how-to/manage-users-teams-and-grants.md) |
| #212 | Added API-team self-service onboarding and platform handoff guidance. | [Onboard an API team](../how-to/onboard-api-team.md) |
| #213 | Made the platform/API-team ownership model explicit for evaluation and rollout. | [Evaluate a production-shaped platform setup](../how-to/evaluate-platform.md), [Onboard an API team](../how-to/onboard-api-team.md) |
| #214 | Removed internal milestone labels from public docs and added a CI guard for future jargon leaks. | [CLI reference](cli.md), [AI gateway route and budget](../how-to/ai-gateway-route-budget.md), [Observability Alert Pack](observability-alerts.md) |
| #215 | Completed final consistency and verification polish for the adoption spine. | This map, [README Documentation](../../README.md#documentation), [Documentation home](../README.md) |
| #216 | Moved observability alert content into the Diataxis reference tree and linked it from platform docs. | [Observability Alert Pack](observability-alerts.md), [Production Readiness](../how-to/production-readiness.md), [Evaluate a production-shaped platform setup](../how-to/evaluate-platform.md) |

The public task pages linked here are completable without a source checkout, `internal/`, `spec/`, or generated files from the repository. Deployment examples listed outside `docs/` are self-contained and do not depend on private artifacts.
