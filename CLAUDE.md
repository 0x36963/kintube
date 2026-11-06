# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

kintube is a private, multi-tenant "YouTube for your own photos and videos": a Rust API plus a React Router v8 SPA.
**PLAN.md is the source of truth** for product scope, architecture decisions and milestones (M0 done; M1 =
uploads → HLS transcode → watch page). When a decision changes, update PLAN.md in the same change. Things the
plan describes but that don't exist yet: `worker`/`media` crates, S3 storage, the job queue (all M1).

## Commands

Run from the repo root unless noted. `make help` lists everything.

```sh
make setup                 # one-time: backend/.env from .env.example, pnpm install, Playwright chromium
make dev-up && make migrate  # Postgres 18 + PostGIS and Mailpit (OrbStack/Docker), then apply migrations
make api                   # API on http://127.0.0.1:8787 (8080 is taken by another app on this machine)
make web                   # SPA on http://localhost:5173, proxies /api → 8787
make check                 # cargo fmt --check, clippy -D warnings, react-router typegen + tsc
make test                  # test-backend (needs dev-up) + test-e2e (needs dev-up + migrate)
make openapi               # regenerate web/openapi.json + web/app/lib/api/schema.d.ts
```

Single tests:

```sh
cd backend && DATABASE_URL=postgres://app:app@localhost:5432/app cargo test -p api --test auth_flow magic_link_works_once
cd backend && cargo test -p app-core auth::tests            # unit tests, no DB needed
cd web && pnpm exec playwright test -g "magic link" --project desktop
```

Dev gotchas:
- Open the app at `localhost:5173`, not `127.0.0.1`. Non-GET requests must send an `Origin` that exactly matches `APP_ORIGIN`.
- Sign-in emails go to Mailpit: UI at http://localhost:8025, JSON API at `/api/v1/...` (the e2e tests read codes from it).
- `api` loads `.env` from the current directory, so run it from `backend/`. `backend/.env` is gitignored; `.env.example` is the template and documents every variable.
- Playwright's `webServer` starts the API and web servers if they aren't already running. Don't start them concurrently with a test run, or the second API fails to bind 8787.
- Local `AUTH_RATE_LIMIT_PER_MINUTE=300` (production default 20) so parallel e2e runs don't trip the per-IP limit.

## Architecture

**Request path.** The browser loads static SPA files. In dev, `/api/*` goes through the Vite proxy; in prod, Cloudflare routes it to the Rust origin. Everything is same-origin, so the session cookie just works. Nothing is server-rendered (`ssr: false`).

**Backend (`backend/`, Cargo workspace)**
- `app-core`: domain logic shared with the future worker: config (env), DB pool + embedded migrator, auth (login tokens, codes, sessions), users, mailer. Business rules go here; HTTP handlers stay thin.
- `api`: a lib (`app(state)` builds the router, `openapi()` the spec) plus a `main.rs` CLI: `api serve | migrate | openapi`. Migrations run only via `migrate`, never at startup.
- Layer order in `api::app`: `TraceLayer` → `require_same_origin` (CSRF: non-GET/HEAD/OPTIONS needs `Origin == APP_ORIGIN`) → `rate_limit_auth` (per-IP, `/api/auth/*` only) → handler.
- Handlers require a signed-in user by taking the `CurrentUser` extractor. Client IP comes from `CLIENT_IP_HEADER` (`cf-connecting-ip` in prod) or the TCP peer.
- Errors are always `{"error": {"code", "message"}}` via `ApiError`. `code` is a stable identifier the frontend branches on (e.g. `login_invalid`, `incorrect_code`), so don't rename existing codes. `AuthError` → `ApiError` mapping lives in `api/src/error.rs`.
- **API contract**: annotate handlers with `#[utoipa::path]`, register them with `routes!()` in the module's `router()`, and merge new modules in `api_router()` (`api/src/lib.rs`). Then run `make openapi` and commit both generated files. CI fails on drift.

**Auth model (passwordless).**
- `POST /api/auth/email/start` creates a `login_tokens` row and emails both a magic link and a 6-digit code. Using either one consumes the row.
- The link token is in the URL **fragment** (`/auth/verify#token=…`). The SPA POSTs it to `verify-link`, so it never hits server logs and mail scanners can't consume it.
- Sign-up = first successful verification (`find_or_create_by_email`).
- Only SHA-256 hashes of tokens, codes and session secrets are stored.
- Sessions are server-side. Cookie is `__Host-sid` (`sid` when `COOKIE_SECURE=false` in dev), `HttpOnly; SameSite=Lax`. Idle expiry is 30 days (slides, written at most every 10 minutes); absolute limit is 365 days. Setting `users.deleted_at` disables the account and invalidates its sessions.

**Database.**
- sqlx **0.9** with runtime-checked `query` / `query_as::<_, T>` + `#[derive(FromRow)]`; there are no compile-time `query!` macros and no sqlx-cli.
- In 0.9 the query functions take `impl SqlSafeStr`, so write SQL as static strings and bind every value.
- Migrations are numbered files in `backend/migrations/`, embedded via `sqlx::migrate!("../../migrations")`. Add new files; never edit applied ones.
- IDs are UUIDv7 generated in the app. Emails are stored normalized (trimmed, lowercase).

**Frontend (`web/`, React Router v8 SPA)**
- Routes are declared in `app/routes.ts`. Signed-in pages are children of `routes/app/layout.tsx`. Its `clientMiddleware` resolves `meQuery` and redirects to `/login?next=…` before any child loader runs. Components read the user with `useUser()`.
- Data: TanStack Query + the typed `api` client (openapi-fetch over the generated schema) + `unwrap()`, which throws `ApiError` with the server's `code`. A 401 from any query clears the cache and sends the user to sign in.
- `app/lib/nav.ts` drives both the sidebar/mobile nav and the shared placeholder page `routes/app/coming-soon.tsx`. When a feature ships, give it its own route module and remove its path from the `comingSoon` list in `routes.ts`.
- Styling: Tailwind v4 with semantic tokens defined in `app/app.css` (`bg-surface`, `text-fg-muted`, `ring-line`, `bg-accent`…). They're backed by CSS variables so a light theme can be added later; use the tokens, not raw colours. Dark-first, YouTube-like layout.
- Privacy product: no third-party requests from the client. Fonts are self-hosted via `@fontsource-variable/inter`.
- Radix primitives come from the unified `radix-ui` package. Icons are lucide-react 1.x (names differ from older docs, e.g. there's no `History`; we use `ClockFading`).
- Branding: name in `app/lib/brand.ts` and `APP_NAME` in `backend/.env`. The logo mark and `--accent` colour are placeholders.

## Testing conventions

- Rust integration tests (`backend/crates/api/tests/`) use `#[sqlx::test(migrator = "app_core::db::MIGRATOR")]`, which gives each test a fresh migrated database. They drive `api::app(state)` with `tower::ServiceExt::oneshot`, using `Mailer::memory()` to capture outgoing email. Reuse the `TestApp` helper: it sets `Origin`, the `x-test-ip` header used as the client-IP header in tests, and cookies.
- Playwright e2e (`web/e2e/`) runs the real stack in `desktop` and `mobile` projects, with a unique email per test.

## Dependency versions

Several dependencies are newer than most training data: React Router 8.5, Vite 8, Tailwind 4, sqlx 0.9, utoipa 6 / utoipa-axum 0.3, axum 0.8, tower-http 0.7, rand 0.10 (`rand::fill`, `rand::random_range`), pnpm 12. Before relying on an API from memory, check the installed source (`~/.cargo/registry/src/…`, `web/node_modules/…/*.d.ts`). On Apple Silicon, the dev Postgres image is `imresamu/postgis`, because the official `postgis/postgis` image is amd64-only.
