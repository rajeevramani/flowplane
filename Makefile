# Flowplane Docker Compose Management
# ===================================
#
# Boot configurations:
#   make up              - Backend + UI
#   make up-mtls         - Backend + UI + Vault (mTLS)
#   make up-tracing      - Backend + UI + Jaeger
#   make up-full         - Backend + UI + Jaeger + Vault + httpbin
#
# Optional services:
#   make up HTTPBIN=1    - Add httpbin to any configuration
#   make up ENVOY=1      - Add platform-admin Envoy proxy
#   make up ENVOY=1 HTTPBIN=1  - Combine multiple options
#
# Operations:
#   make down            - Stop all services
#   make logs            - Tail logs from all services
#   make status          - Show running containers
#   make clean           - Remove volumes and orphan containers

.PHONY: help up up-mtls up-tracing up-full down logs status clean \
        build build-backend build-ui info prune \
        vault-setup dev-db test test-ui test-ui-watch test-ui-e2e test-ui-report \
        test-e2e test-e2e-full test-e2e-mtls test-cleanup fmt clippy check

.DEFAULT_GOAL := help

# Colors
CYAN := \033[36m
GREEN := \033[32m
YELLOW := \033[33m
RED := \033[31m
RESET := \033[0m

# Docker/Podman detection
# Use podman if docker is not available
DOCKER := $(shell command -v docker 2>/dev/null || command -v podman 2>/dev/null)
DOCKER_COMPOSE := $(shell command -v docker-compose 2>/dev/null || command -v podman-compose 2>/dev/null)

# Docker BuildKit (ignored by podman)
export DOCKER_BUILDKIT := 1
export COMPOSE_DOCKER_CLI_BUILD := 1

# Project info
VERSION := $(shell grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
IMAGE_NAME := flowplane
BACKEND_IMAGE := flowplane-backend
UI_IMAGE := flowplane-ui

# Base compose files for 'up' command
BASE_COMPOSE := -f docker-compose.yml

# Conditional httpbin - adds to any configuration
ifdef HTTPBIN
    HTTPBIN_COMPOSE := -f docker-compose-httpbin.yml
else
    HTTPBIN_COMPOSE :=
endif

# Conditional envoy - adds platform-admin Envoy proxy to any configuration
ifdef ENVOY
    ENVOY_COMPOSE := -f docker-compose-envoy.yml
else
    ENVOY_COMPOSE :=
endif

# =============================================================================
# Help
# =============================================================================

help: ## Show this help message
	@echo "$(CYAN)Flowplane Docker Compose Commands$(RESET)"
	@echo ""
	@echo "$(GREEN)Boot Configurations:$(RESET)"
	@echo "  $(CYAN)make up$(RESET)              - Backend + UI"
	@echo "  $(CYAN)make up-mtls$(RESET)         - Backend + UI + Vault (mTLS)"
	@echo "  $(CYAN)make up-tracing$(RESET)      - Backend + UI + Jaeger"
	@echo "  $(CYAN)make up-full$(RESET)         - Backend + UI + Jaeger + Vault + httpbin"
	@echo ""
	@echo "$(GREEN)Optional Services:$(RESET)"
	@echo "  $(CYAN)HTTPBIN=1$(RESET)            - Add httpbin (e.g., make up HTTPBIN=1)"
	@echo "  $(CYAN)ENVOY=1$(RESET)              - Add platform-admin Envoy proxy (e.g., make up ENVOY=1)"
	@echo ""
	@echo "$(GREEN)Build Targets:$(RESET)"
	@echo "  $(CYAN)make build$(RESET)           - Build combined Docker image"
	@echo "  $(CYAN)make build-backend$(RESET)   - Build backend-only image (optimized)"
	@echo "  $(CYAN)make build-ui$(RESET)        - Build frontend-only image"
	@echo ""
	@echo "$(GREEN)Operations:$(RESET)"
	@echo "  $(CYAN)make down$(RESET)            - Stop all services"
	@echo "  $(CYAN)make logs$(RESET)            - Tail logs from all services"
	@echo "  $(CYAN)make status$(RESET)          - Show running containers"
	@echo "  $(CYAN)make clean$(RESET)           - Remove volumes and orphans"
	@echo "  $(CYAN)make prune$(RESET)           - Aggressive cleanup (images + cache)"
	@echo "  $(CYAN)make info$(RESET)            - Show disk usage and image sizes"
	@echo ""
	@echo "$(GREEN)Development:$(RESET)"
	@echo "  $(CYAN)make dev-db$(RESET)          - Start PostgreSQL for local dev"
	@echo "  $(CYAN)make test$(RESET)            - Run cargo tests"
	@echo "  $(CYAN)make test-ui$(RESET)         - Run UI component tests (Vitest)"
	@echo "  $(CYAN)make test-ui-watch$(RESET)   - Run UI tests in watch mode"
	@echo "  $(CYAN)make test-ui-e2e$(RESET)     - Run UI E2E tests (Playwright)"
	@echo "  $(CYAN)make test-e2e$(RESET)        - Run E2E smoke tests (cleanup containers after)"
	@echo "  $(CYAN)make test-e2e-full$(RESET)   - Run full E2E suite with mTLS (cleanup after)"
	@echo "  $(CYAN)make test-e2e-mtls$(RESET)   - Run mTLS E2E tests only (cleanup after)"
	@echo "  $(CYAN)make test-cleanup$(RESET)    - Remove orphaned testcontainer containers"
	@echo "  $(CYAN)make fmt$(RESET)             - Run cargo fmt"
	@echo "  $(CYAN)make clippy$(RESET)          - Run cargo clippy"
	@echo "  $(CYAN)make check$(RESET)           - Run fmt + clippy + test"
	@echo "  $(CYAN)make vault-setup$(RESET)     - Run Vault PKI setup script"
	@echo ""
	@echo "$(GREEN)Examples:$(RESET)"
	@echo "  $(CYAN)make up-tracing HTTPBIN=1$(RESET)  - Backend + Jaeger + httpbin"
	@echo "  $(CYAN)make up ENVOY=1 HTTPBIN=1$(RESET)  - Backend + Envoy + httpbin"
	@echo "  $(CYAN)make up-full$(RESET)               - Full stack with all services"

# =============================================================================
# Boot Configurations
# =============================================================================

up: _ensure-network ## Start backend + UI
	@echo "$(CYAN)Starting Flowplane (Backend + UI)...$(RESET)"
	$(DOCKER_COMPOSE) $(BASE_COMPOSE) $(HTTPBIN_COMPOSE) $(ENVOY_COMPOSE) up -d
	@echo ""
	@echo "$(GREEN)Services started!$(RESET)"
	@echo "  API:        http://localhost:8080/api/v1/"
	@echo "  UI:         http://localhost:8080/"
	@echo "  Swagger:    http://localhost:8080/swagger-ui/"
	@echo "  xDS gRPC:   localhost:50051"
ifdef HTTPBIN
	@echo "  httpbin:    http://localhost:8000"
endif
ifdef ENVOY
	@echo "  Envoy (platform-admin):"
	@echo "    Listener: http://localhost:10000"
	@echo "    Admin:    http://localhost:9901"
endif

up-mtls: _ensure-network ## Start backend + UI + Vault (mTLS)
	@echo "$(CYAN)Starting Flowplane with Vault (mTLS)...$(RESET)"
	$(DOCKER_COMPOSE) $(BASE_COMPOSE) -f docker-compose-mtls-dev.yml $(HTTPBIN_COMPOSE) $(ENVOY_COMPOSE) up -d
	@echo ""
	@echo "$(GREEN)Services started!$(RESET)"
	@echo "  API:        http://localhost:8080/api/v1/"
	@echo "  UI:         http://localhost:8080/"
	@echo "  Swagger:    http://localhost:8080/swagger-ui/"
	@echo "  xDS gRPC:   localhost:50051"
	@echo "  Vault UI:   http://localhost:8200 (token: flowplane-dev-token)"
ifdef HTTPBIN
	@echo "  httpbin:    http://localhost:8000"
endif
ifdef ENVOY
	@echo "  Envoy (platform-admin):"
	@echo "    Listener: http://localhost:10000"
	@echo "    Admin:    http://localhost:9901"
endif
	@echo ""
	@echo "$(YELLOW)Next: Run PKI setup$(RESET)"
	@echo "  make vault-setup"

up-tracing: _ensure-network ## Start backend + UI + Jaeger
	@echo "$(CYAN)Starting Flowplane with Jaeger (tracing)...$(RESET)"
	$(DOCKER_COMPOSE) -f docker-compose-jaeger.yml $(HTTPBIN_COMPOSE) $(ENVOY_COMPOSE) up -d
	@echo ""
	@echo "$(GREEN)Services started!$(RESET)"
	@echo "  API:        http://localhost:8080/api/v1/"
	@echo "  UI:         http://localhost:8080/"
	@echo "  Swagger:    http://localhost:8080/swagger-ui/"
	@echo "  xDS gRPC:   localhost:50051"
	@echo "  Jaeger UI:  http://localhost:16686"
ifdef HTTPBIN
	@echo "  httpbin:    http://localhost:8000"
endif
ifdef ENVOY
	@echo "  Envoy (platform-admin):"
	@echo "    Listener: http://localhost:10000"
	@echo "    Admin:    http://localhost:9901"
endif

up-full: _ensure-network ## Start backend + UI + Jaeger + Vault (full stack)
	@echo "$(CYAN)Starting Flowplane (Full Stack)...$(RESET)"
	$(DOCKER_COMPOSE) -f docker-compose-secrets-tracing.yml up -d
	@echo ""
	@echo "$(GREEN)Services started!$(RESET)"
	@echo "  API:        http://localhost:8080/api/v1/"
	@echo "  UI:         http://localhost:8080/"
	@echo "  Swagger:    http://localhost:8080/swagger-ui/"
	@echo "  xDS gRPC:   localhost:50051"
	@echo "  Vault UI:   http://localhost:8200 (token: flowplane-dev-token)"
	@echo "  Jaeger UI:  http://localhost:16686"
	@echo "  httpbin:    http://localhost:8000"

# =============================================================================
# Container Operations
# =============================================================================

down: ## Stop all services
	@echo "$(CYAN)Stopping all Flowplane services...$(RESET)"
	-$(DOCKER_COMPOSE) $(BASE_COMPOSE) down 2>/dev/null || true
	-$(DOCKER_COMPOSE) $(BASE_COMPOSE) -f docker-compose-mtls-dev.yml down 2>/dev/null || true
	-$(DOCKER_COMPOSE) -f docker-compose-jaeger.yml down 2>/dev/null || true
	-$(DOCKER_COMPOSE) -f docker-compose-secrets-tracing.yml down 2>/dev/null || true
	-$(DOCKER_COMPOSE) -f docker-compose-httpbin.yml down 2>/dev/null || true
	-$(DOCKER_COMPOSE) $(BASE_COMPOSE) -f docker-compose-envoy.yml down 2>/dev/null || true
	-$(DOCKER_COMPOSE) -f docker-compose-monitoring.yml down 2>/dev/null || true
	@echo "$(GREEN)All services stopped.$(RESET)"

logs: ## Tail logs from all running services
	@echo "$(CYAN)Tailing logs (Ctrl+C to exit)...$(RESET)"
	@$(DOCKER) logs -f flowplane-control-plane 2>/dev/null || echo "$(YELLOW)No control-plane container running$(RESET)"

status: ## Show running Flowplane containers
	@echo "$(CYAN)Flowplane Services Status:$(RESET)"
	@$(DOCKER) ps --filter "name=flowplane-" --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}" 2>/dev/null || echo "No containers running"

clean: down ## Remove volumes and orphan containers
	@echo "$(YELLOW)Cleaning up volumes and orphans...$(RESET)"
	-$(DOCKER_COMPOSE) $(BASE_COMPOSE) down -v --remove-orphans 2>/dev/null || true
	-$(DOCKER_COMPOSE) -f docker-compose-jaeger.yml down -v --remove-orphans 2>/dev/null || true
	-$(DOCKER_COMPOSE) -f docker-compose-secrets-tracing.yml down -v --remove-orphans 2>/dev/null || true
	@echo "$(GREEN)Cleanup complete.$(RESET)"

# =============================================================================
# Build Targets
# =============================================================================

build: ## Build combined Docker image
	@echo "$(CYAN)Building combined Docker image...$(RESET)"
	$(DOCKER) build -t $(IMAGE_NAME):$(VERSION) -t $(IMAGE_NAME):latest .
	@echo "$(GREEN)Image built: $(IMAGE_NAME):$(VERSION)$(RESET)"

build-backend: ## Build backend-only Docker image (optimized with cargo-chef)
	@echo "$(CYAN)Building backend Docker image...$(RESET)"
	$(DOCKER) build -f Dockerfile.backend -t $(BACKEND_IMAGE):$(VERSION) -t $(BACKEND_IMAGE):latest .
	@echo "$(GREEN)Image built: $(BACKEND_IMAGE):$(VERSION)$(RESET)"

build-ui: ## Build frontend-only Docker image
	@echo "$(CYAN)Building UI Docker image...$(RESET)"
	$(DOCKER) build -f Dockerfile.ui -t $(UI_IMAGE):$(VERSION) -t $(UI_IMAGE):latest .
	@echo "$(GREEN)Image built: $(UI_IMAGE):$(VERSION)$(RESET)"

# =============================================================================
# Development Targets
# =============================================================================

dev-db: _ensure-network ## Start PostgreSQL for local development
	@echo "$(CYAN)Starting PostgreSQL...$(RESET)"
	$(DOCKER_COMPOSE) $(BASE_COMPOSE) up -d postgres
	@echo "$(GREEN)PostgreSQL running on localhost:5432$(RESET)"
	@echo "  URL: postgresql://flowplane:flowplane@localhost:5432/flowplane"

test: ## Run cargo tests (requires Docker/Podman for testcontainers)
	@echo "$(CYAN)Running tests...$(RESET)"
	cargo test --features postgres_tests

test-ui: ## Run UI component tests (Vitest)
	@echo "$(CYAN)Running UI component tests...$(RESET)"
	cd ui && npx vitest run

test-ui-watch: ## Run UI component tests in watch mode
	@echo "$(CYAN)Running UI tests in watch mode...$(RESET)"
	cd ui && npx vitest

test-ui-e2e: ## Run UI E2E tests (Playwright, requires running backend)
	@echo "$(CYAN)Running UI E2E tests...$(RESET)"
	cd ui && npx playwright test
	@echo ""
	@echo "$(GREEN)HTML report: make test-ui-report$(RESET)"

test-ui-report: ## Open Playwright HTML test report
	cd ui && npx playwright show-report

test-e2e: ## Run E2E smoke tests and clean up containers
	@echo "$(CYAN)Running E2E smoke tests...$(RESET)"
	RUN_E2E=1 RUST_LOG=info cargo test -p flowplane --test e2e smoke -- --ignored --nocapture --test-threads=1; \
	TEST_EXIT=$$?; \
	$(MAKE) test-cleanup; \
	exit $$TEST_EXIT

test-e2e-full: ## Run full E2E suite and clean up containers
	@echo "$(CYAN)Running full E2E suite...$(RESET)"
	RUN_E2E=1 RUST_LOG=info FLOWPLANE_E2E_MTLS=1 cargo test -p flowplane --test e2e -- --ignored --nocapture --test-threads=1; \
	TEST_EXIT=$$?; \
	$(MAKE) test-cleanup; \
	exit $$TEST_EXIT

test-e2e-mtls: ## Run mTLS E2E tests and clean up containers
	@echo "$(CYAN)Running mTLS E2E tests...$(RESET)"
	FLOWPLANE_E2E_MTLS=1 RUN_E2E=1 RUST_LOG=info cargo test --test e2e "test_24_mtls" -- --ignored --nocapture --test-threads=1; \
	TEST_EXIT=$$?; \
	$(MAKE) test-cleanup; \
	exit $$TEST_EXIT

test-cleanup: ## Remove orphaned testcontainer PostgreSQL containers
	@CONTAINERS=$$($(DOCKER) ps -q --filter "label=org.testcontainers.managed-by=testcontainers" --filter "ancestor=postgres" 2>/dev/null); \
	if [ -n "$$CONTAINERS" ]; then \
		echo "$(YELLOW)Stopping $$(echo "$$CONTAINERS" | wc -l | tr -d ' ') testcontainer(s)...$(RESET)"; \
		$(DOCKER) stop --time 5 $$CONTAINERS 2>/dev/null || true; \
		$(DOCKER) rm -f $$CONTAINERS 2>/dev/null || true; \
		echo "$(GREEN)Testcontainers cleaned up.$(RESET)"; \
	else \
		echo "$(GREEN)No orphaned testcontainers found.$(RESET)"; \
	fi

fmt: ## Run cargo fmt
	@echo "$(CYAN)Running cargo fmt...$(RESET)"
	cargo fmt --all

clippy: ## Run cargo clippy
	@echo "$(CYAN)Running cargo clippy...$(RESET)"
	cargo clippy --all-targets --all-features -- -D warnings

check: fmt clippy test test-ui ## Run fmt + clippy + test + UI tests
	@echo "$(GREEN)All checks passed!$(RESET)"

vault-setup: ## Run Vault PKI setup script
	@echo "$(CYAN)Setting up Vault PKI...$(RESET)"
	@if [ -f ./scripts/setup-vault-pki.sh ]; then \
		./scripts/setup-vault-pki.sh; \
	else \
		echo "$(RED)Error: scripts/setup-vault-pki.sh not found$(RESET)"; \
		exit 1; \
	fi

# =============================================================================
# Information & Cleanup
# =============================================================================

info: ## Show disk usage and image sizes
	@echo "$(CYAN)=== Docker Disk Usage ===$(RESET)"
	@$(DOCKER) system df
	@echo ""
	@echo "$(CYAN)=== Flowplane Images ===$(RESET)"
	@$(DOCKER) images | grep -E "flowplane|REPOSITORY" || echo "No flowplane images found"
	@echo ""
	@echo "$(CYAN)=== Flowplane Volumes ===$(RESET)"
	@$(DOCKER) volume ls | grep -E "flowplane|DRIVER" || echo "No flowplane volumes found"
	@echo ""
	@echo "$(CYAN)=== Project Info ===$(RESET)"
	@echo "Version: $(VERSION)"

prune: down ## Aggressive cleanup (images + volumes + cache)
	@echo "$(RED)WARNING: Removing all flowplane images and build cache$(RESET)"
	@echo "$(YELLOW)Press Ctrl+C to cancel, waiting 3 seconds...$(RESET)"
	@sleep 3
	@echo "$(YELLOW)Removing images...$(RESET)"
	-$(DOCKER) rmi $$($(DOCKER) images | grep flowplane | awk '{print $$3}') 2>/dev/null || true
	@echo "$(YELLOW)Removing volumes...$(RESET)"
	-$(DOCKER) volume rm $$($(DOCKER) volume ls -q | grep flowplane) 2>/dev/null || true
	@echo "$(YELLOW)Pruning build cache...$(RESET)"
	$(DOCKER) builder prune -af
	@echo "$(GREEN)Cleanup complete.$(RESET)"

# =============================================================================
# Internal Targets
# =============================================================================

_ensure-network:
	@$(DOCKER) network inspect flowplane-network >/dev/null 2>&1 || \
		$(DOCKER) network create flowplane-network >/dev/null 2>&1
