# kintube

A private, multi-tenant "YouTube for your own photos and videos". See [PLAN.md](PLAN.md) for
the product and architecture plan.

| Path | What |
|---|---|
| `backend/` | Rust workspace: `api` (Axum HTTP server + CLI), `app-core` (domain logic, DB, auth, email) |
| `backend/migrations/` | SQL migrations, embedded into the binary and applied with `make migrate` |
| `web/` | React Router v8 SPA (`ssr: false`), Tailwind v4, TanStack Query, typed API client |
| `infra/compose.dev.yml` | Local Postgres 18 + PostGIS and Mailpit |

## Prerequisites

Rust (stable), Node ≥ 22.22 with pnpm, and Docker (OrbStack on macOS).

## Getting started

```sh
make setup     # backend/.env, JS deps, Playwright browser
make dev-up    # Postgres + Mailpit
make migrate
make api       # terminal 1: http://127.0.0.1:8787
make web       # terminal 2: http://localhost:5173
```

Open http://localhost:5173 and sign in with any email address. The sign-in email (link + code)
lands in Mailpit at http://localhost:8025. Use `localhost`, not `127.0.0.1`: the API only accepts
state-changing requests whose `Origin` matches `APP_ORIGIN`.

## Everyday commands

```sh
make check     # rustfmt, clippy, TypeScript
make test      # Rust unit/integration tests + Playwright e2e (desktop and mobile)
make openapi   # regenerate web/app/lib/api/schema.d.ts after changing API types
make help      # everything else
```

The API's OpenAPI document is served at `/api/openapi.json`. CI fails if the generated TypeScript
client is out of date.

## Branding

The product name lives in `web/app/lib/brand.ts` and `APP_NAME` in `backend/.env`. The logo mark
(`web/public/favicon.svg`, `Logo` in `web/app/components/ui.tsx`) and accent colour
(`--accent` in `web/app/app.css`) are still placeholders.
