# Local development. Run `make help` for the list.
DATABASE_URL ?= postgres://app:app@localhost:5432/app
COMPOSE := docker compose -f infra/compose.dev.yml

.PHONY: help setup dev-up dev-down dev-reset migrate api worker web openapi test test-backend test-e2e test-edge check

help: ## Show this help
	@grep -E '^[a-z0-9-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  \033[36m%-13s\033[0m %s\n", $$1, $$2}'

setup: ## One-time: env file, JS deps, Playwright browser
	@test -f backend/.env || cp backend/.env.example backend/.env
	cd web && pnpm install && pnpm exec playwright install chromium
	cd edge && pnpm install

dev-up: ## Start Postgres (PostGIS), S3 (RustFS, console :9001) and Mailpit (UI: http://localhost:8025)
	$(COMPOSE) up -d --wait

dev-down: ## Stop dev services (data is kept)
	$(COMPOSE) down

dev-reset: ## Stop dev services and DELETE their data
	$(COMPOSE) down -v

migrate: ## Apply database migrations and configure the S3 bucket (CORS, lifecycle)
	cd backend && cargo run -q -p api -- migrate && cargo run -q -p api -- storage-setup

api: ## Run the API on http://127.0.0.1:8787
	cd backend && cargo run -p api -- serve

worker: ## Run the media worker (needs ffmpeg on PATH)
	cd backend && cargo run -p worker

web: ## Run the web app on http://localhost:5173 (proxies /api to the API)
	cd web && pnpm run dev

openapi: ## Regenerate the TypeScript API client from the Rust OpenAPI spec
	cd web && pnpm run gen:api

test: test-backend test-edge test-e2e ## Run all tests

test-backend: ## Rust unit + integration tests (needs dev-up and ffmpeg)
	cd backend && DATABASE_URL=$(DATABASE_URL) cargo test

test-e2e: ## Playwright end-to-end tests (needs dev-up + migrate; starts api/worker/web)
	cd web && pnpm run test:e2e

test-edge: ## Media gateway (Cloudflare Worker) tests
	cd edge && pnpm test

check: ## Formatting, lints, type checks
	cd backend && cargo fmt --all --check && cargo clippy --all-targets -- -D warnings
	cd web && pnpm run typecheck
	cd edge && pnpm run typecheck
