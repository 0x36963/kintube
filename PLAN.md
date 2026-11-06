# kintube — Project Plan

A private, multi-tenant "YouTube for your own photos and videos." Users sign up, upload their media, and browse it in a familiar, dark-first, YouTube-style UI. Content is **private by default**, and users can choose to share it.

---

## 1. Decisions

| Area | Decision | Source |
|---|---|---|
| Product | Multi-tenant SaaS, public internet, open sign-up | You |
| Library scale | Unbounded (from hundreds to hundreds of thousands of items per user) | You |
| File management | Full: upload, edit, move, delete (with trash) | You |
| Storage | S3-compatible: **Cloudflare R2** in prod, **MinIO** in dev | You |
| Video playback | Adaptive **HLS** ladder (CMAF/fMP4), quality picker, **up to 4K (2160p) in v1** | You |
| Default quota | **20 GB** storage per user. File-size and processing limits are tied to it (§4) | You set storage; the derived limits are my proposal |
| Database ops | **Self-managed Postgres** on Hetzner (§7.1) | You |
| Local dev | **OrbStack** + `docker compose` | You |
| Sharing | Private by default, plus unlisted links, user-to-user grants, shared albums/channels | You |
| Privacy | **Hybrid**: normal items are encrypted at rest (server-trusted); a per-item **Vault** is E2E encrypted | You |
| Backend | **Rust**: Axum + Tokio + SQLx + Postgres (PostGIS) | You |
| Frontend | **React Router v8**, **SPA mode** (`ssr: false`) | You chose RR v8; SPA mode is my call (§3) |
| Auth | Magic links (with OTP code), Passkeys, OAuth (Google, Apple). No passwords | You |
| Billing | Quotas enforced from day 1, Stripe later | You |
| Smart/AI | None in v1. The schema leaves room for pgvector, faces and transcripts | You |
| Mobile | Responsive web + PWA. Native auto-backup app later | You |
| Hosting | Hetzner/OVH boxes (API and transcode workers) + R2 + Cloudflare edge | You |
| Design | Familiar YouTube-like layout, dark-first, own branding | You |
| Milestone 1 | Video vertical slice: sign in → upload → HLS transcode → watch with scrubbing | You |

React Router v8 baseline (shipped 2026-06-17): Node ≥ 22.22, React ≥ 19.2.7, Vite ≥ 7. It is ESM-only, has no `react-router-dom`, and middleware is always on. Local Node is v24.3 ✅.

---

## 2. Architecture

```
                    ┌──────────────────────── Cloudflare ────────────────────────┐
 Browser / PWA ───► │  app.example.com                                            │
                    │    /*        → static SPA (React Router v8, ssr:false)      │
                    │    /s/:token → Worker injects OG tags into SPA shell        │
                    │    /api/*    → proxied to Rust API origin                   │
                    │                                                             │
                    │  media.example-usercontent.com   (separate, cookieless)     │
                    │    /{token}/… → Media Gateway Worker ─► R2 bucket           │
                    └────────────────────────────────┬───────────────────────────┘
                                                     │
        direct multipart upload (presigned) ─────────┼──────────────► R2
                                                     ▼
                       ┌──────────── Hetzner ────────────┐
                       │  api   (Axum)   ── Postgres ◄───┤── worker(s): ffmpeg,
                       │                   + PostGIS     │   ffprobe, libvips,
                       │                   + job queue   │   exiftool (sandboxed)
                       └─────────────────────────────────┘
```

### Components

| Component | Tech | Responsibility |
|---|---|---|
| `web/` | React Router v8 SPA, Vite 7, TS, Tailwind v4, Radix primitives, TanStack Query + Virtual, Vidstack (hls.js), Uppy, MapLibre | All UI |
| `backend/crates/api` | Axum, tower-http, SQLx, utoipa (OpenAPI), webauthn-rs, openidconnect | HTTP API, auth, authz, upload orchestration, token minting |
| `backend/crates/worker` | Tokio + subprocess ffmpeg/vips/exiftool | Media processing jobs, purge jobs, maintenance |
| `backend/crates/app-core` | SQLx, aws-sdk-s3 | Domain types, DB access, storage abstraction, job queue, authz checks |
| `backend/crates/media` | — | Typed wrappers for ffprobe/ffmpeg/vips/exiftool and the HLS ladder logic |
| `edge/` | Cloudflare Worker (TS) | Media gateway (token check, R2 reads, Range, caching) and OG injection for share pages |
| Postgres 16+ | + PostGIS, pg_trgm | All metadata, FTS, geo, job queue (`FOR UPDATE SKIP LOCKED`, so no Redis) |

### Why SPA mode (my call)
- All content is private, so server-side rendering earns no SEO.
- The only HTML that must be rendered server-side is share-link unfurls (Open Graph). An edge Worker handles that with `HTMLRewriter`. Marketing pages can use RR's build-time `prerender`.
- **One backend runtime** (Rust) to deploy, secure and scale. A Node SSR tier would add a hop for every API call and buy little.
- The frontend becomes static files on Cloudflare, so frontend hosting costs almost nothing.
- RR v8 still provides file routes, `clientLoader`/`clientAction` and client middleware. TanStack Query sits underneath for caching, infinite scroll and optimistic updates (`clientLoader` → `queryClient.ensureQueryData`).

### Type safety across the language boundary
Rust handlers are annotated with `utoipa`, which produces `openapi.json`. `openapi-typescript` and `openapi-fetch` turn that into a typed client in `web/`. CI fails if the generated client is stale.

---

## 3. Core pipelines

### 3.1 Upload (resumable, direct to R2)
1. `POST /api/uploads` sends `{filename, size, mime}`. The server checks quota and allowed type, **reserves** the bytes against the quota, creates a `media` row (`status=uploading`), starts an S3 multipart upload and returns `{media_id, upload_id, part_size}`.
2. The client (Uppy AwsS3 multipart) requests presigned part URLs in batches and uploads parts in parallel, straight to R2. Interrupted uploads can resume.
3. `POST /api/uploads/:id/complete` completes the multipart upload, sets `status=processing` and enqueues a `process_media` job.
4. If the upload is aborted or abandoned, the quota reservation is released. An R2 lifecycle rule aborts incomplete multipart uploads after 7 days.
- Gotchas: R2 CORS must allow `PUT` and **expose `ETag`**. Part size scales with file size because S3 allows at most 10,000 parts. The worker sniffs magic bytes and does not trust the client's MIME type.
- A global upload tray persists across route changes, so users can browse while uploads run.

### 3.2 Video processing (`process_media`, kind=video)
1. **Probe**: ffprobe reads duration, codecs, dimensions, rotation, HDR flags, `creation_time` and QuickTime GPS.
2. **Instant play**: if the original is browser-compatible (H.264/AAC MP4), it is playable immediately with a "HD processing…" badge.
3. **Poster**: ffmpeg's `thumbnail` filter picks a frame, which is encoded to WebP at 3 sizes, plus a thumbhash placeholder.
4. **HLS ladder**: one ffmpeg run with `split` and `-var_stream_map`.
   - H.264 High + AAC 128k, fMP4/CMAF segments, 6 s segments, keyframes forced every 2 s, `-sc_threshold 0`.
   - Rungs are limited to the source resolution and keyed on the **short side**, so vertical video works: 2160 / 1440 / 1080 / 720 / 480 / 360.
   - With 4K on, a 2160p source costs about **4×** the encode work of a 1080p source (output pixel area: about 15.6 vs 3.6 Mpx per frame across the ladder). To keep that manageable:
     - Use faster x264 presets for the 2160p and 1440p rungs.
     - Benchmark hardware encoding (QSV/NVENC) during M1 instead of after v1.
     - Size the worker boxes for it.
   - HDR is tonemapped to SDR (`zscale` + `tonemap`) for v1.
5. **Scrub sprites**: one frame every `max(1s, duration/200)`, 160 px wide, tiled 10×10 per sheet, plus a WebVTT thumbnails track with `#xywh=`.
6. **Hover preview** (M2): three ~1.5 s muted clips joined into one 320 px MP4.
7. Results go to R2, the `media_assets` rows are written and the media becomes `ready`. Progress is pushed to the UI over SSE (`/api/events`).

Multi-tenant fairness: the job queue serves users **round-robin**, so one person's 2,000-file import cannot starve everyone else. Per-job timeouts, heartbeats, retries with backoff and a dead-letter state complete the queue design.

### 3.3 Photo processing (`process_media`, kind=photo), M3
- **Metadata**: `exiftool` in `-stay_open` mode reads capture time (with timezone), GPS, camera, orientation and the Apple `ContentIdentifier`.
- **Derivatives**: libvips (with libheif) makes WebP at 256 / 1024 / 2048 plus a thumbhash, with EXIF orientation applied. Very large panoramas are handled by vips streaming.
- **RAW** (DNG/CR3/NEF/ARW…): derivatives use the embedded preview JPEG via exiftool, falling back to libraw. The original is kept for download.
- **Live Photos**: the HEIC still and the MOV are paired by `ContentIdentifier`. The MOV is hidden from grids and plays on hover or press-and-hold.

### 3.4 Private media delivery
- The API mints short-lived **path tokens**: `HMAC-SHA256(secret, media_id | scope | exp)`.
- URL shape: `https://media.example-usercontent.com/{token}/m/{media_id}/hls/master.m3u8`. HLS playlists use **relative** segment URLs, so every segment inherits the token. The playlists stay static and cacheable.
- The gateway Worker verifies the token, reads from the R2 binding, supports `Range` and caches by object key with the token stripped.
- The **separate cookieless domain** means a malicious upload can never run script on the app origin. The gateway always sends `nosniff`, and non-media originals get `Content-Disposition: attachment`.
- Dev: the Rust API serves the same `/{token}/…` paths from MinIO. Both implementations are checked against shared HMAC test vectors.

---

## 4. Data model (first pass)

```
users(id, email, display_name, avatar_key, storage_limit, storage_used,
      storage_reserved, transcode_minutes_limit, role, created_at, deleted_at)
identities(user_id, provider, subject)              -- google/apple
passkeys(user_id, credential_id, public_key, sign_count, name, created_at)
sessions(id_hash, user_id, user_agent, ip, created_at, last_seen, expires_at)
login_tokens(token_hash, email, otp_hash, expires_at, used_at)  -- magic link + 6-digit code

media(id, owner_id, kind[photo|video], status[uploading|processing|ready|failed],
      title, description, original_key, original_filename, mime, bytes, sha256,
      width, height, duration_ms, taken_at, taken_tz, location geography(Point),
      exif jsonb, live_pair_id, channel_id,
      encryption[none|e2e], encrypted_meta bytea,          -- Vault (M5)
      search tsvector, created_at, trashed_at)
media_assets(media_id, kind[hls|poster|thumb|sprite|preview|…], key, width, bytes)

channels(id, owner_id, handle, name, description, avatar_key, banner_key)
collections(id, owner_id, kind[album|playlist], title, description, cover_media_id,
            ordering[manual|taken_at])
collection_items(collection_id, media_id, position, added_by)

grants(resource_type, resource_id, grantee_user_id, role[viewer|contributor|editor])
share_links(id, token_hash, resource_type, resource_id, created_by, expires_at,
            password_hash, allow_download, strip_location, revoked_at, view_count)

chapters(media_id, start_ms, title)        -- parsed from "0:00 Intro" lines in description
watch_progress(user_id, media_id, position_ms, completed, updated_at)
comments(id, media_id, author_id, parent_id, body, at_ms, created_at, edited_at, deleted_at)
reactions(media_id, user_id, kind)

jobs(id, kind, payload jsonb, owner_id, status, attempts, run_at, locked_by,
     locked_until, last_error)
reports(id, resource_type, resource_id, reporter, reason, status)
audit_log(id, actor_id, action, target, meta, created_at)
```

- IDs are UUIDv7 (time-ordered). R2 keys follow `u/{owner_id}/m/{media_id}/{original|hls/…|thumb/…}`.
- **Authz has one choke point**: `core::authz::can(actor, action, resource)`. Every query goes through it, and integration tests assert that **every endpoint rejects cross-tenant access**. Postgres RLS gets evaluated as a hardening layer in M6.
- **Quota & limits** (all per-plan columns, so Stripe tiers can change them later):
  - **Storage: 20 GB.** User-facing usage counts originals only. Derived bytes are tracked internally. H.264 renditions need more bits than iPhone HEVC originals, so with 4K on, 1 GB of quota can cost about 2–2.5 GB of real R2 storage. Price tiers with that in mind.
  - **Max file size**: limited by remaining quota, with a **50 GB** hard cap per file. That cap matters for future paid tiers; R2 multipart goes far higher.
  - **Max video duration: 4 h**, which keeps any single transcode job bounded.
  - **Processing: 600 processing-minutes/month.** A processing-minute is one source minute × resolution weight (≤1080p ×1, 1440p ×2, 2160p ×4).
    - That covers filling the whole 20 GB with iPhone 4K30 footage in one month (≈170 MB/min → ≈118 min × 4 ≈ 470) with headroom.
    - Its real job is to stop upload/delete churn from consuming unlimited CPU.
    - It's hidden in the UI until a user passes 80%. Reprocessing that we trigger (e.g. a ladder change) never counts.
- Timeline at scale: `GET /api/timeline/buckets` returns month buckets with counts. The client lays out placeholder heights and fetches each bucket as it scrolls into view. Together with virtualization, this keeps 100k+ item libraries smooth.

---

## 5. Frontend

### Routes
| Route | Page |
|---|---|
| `/` | Home: Continue watching, Recent uploads, shelves per channel, "On this day" |
| `/photos` | Timeline: justified grid by day, date scrubber rail, multi-select bulk actions |
| `/videos` | Video grid (YouTube-style cards: thumbnail, duration, hover preview) |
| `/watch/:id` | Player, title, description with chapter links, comments, Up-next rail (playlist → channel → same day) |
| `/photo/:id` | Lightbox (modal over the timeline; deep-linkable); Live Photo playback, EXIF panel, mini-map |
| `/c/:handle` | Channel page: banner, tabs (Videos · Photos · Playlists · About) |
| `/playlist/:id`, `/album/:id` | Collections |
| `/map` | MapLibre + supercluster; tiles from self-hosted Protomaps PMTiles on R2 |
| `/history`, `/liked`, `/shared` | Library pages |
| `/search` | FTS + filters (type, date range, channel, has-location) |
| `/s/:token` | Public share view (minimal chrome, report button) |
| `/settings/*` | Account, passkeys, linked logins, sessions, storage usage, export/delete account |
| `/vault` | M5 |
| `/admin/*` | Users, quotas, jobs, reports/takedowns |

### UX details
- Layout: collapsible left sidebar, top bar with search and an upload button, responsive card grid. Dark theme by default with a light theme available. Mobile gets a bottom nav.
- Player keyboard shortcuts: `k`/space, `j`/`l`, ←/→, `f`, `m`, `t` (theater), `,`/`.` (frame step), `0`–`9`.
- Player features: resume prompt, autoplay-next toggle, playback speed, quality menu, chapters on the seek bar, sprite previews.
- Comments can include timestamps (`1:23`), and clicking one seeks the player.
- Full management: inline title/description edit, change capture date, move to channel, add to album or playlist, bulk select, download original, trash with 30-day restore.
- PWA: installable, cached app shell. On Android, the Web Share Target lets users upload from the system share sheet. This is not available on iOS.

---

## 6. Security, privacy and abuse

- **Sessions**: opaque random tokens, stored hashed. Cookies are `HttpOnly; Secure; SameSite=Lax`. State-changing requests require a JSON content-type and an `Origin` check.
- **Auth**:
  - Each magic-link email also contains a 6-digit code, for when the link opens on another device.
  - The link token travels in the URL **fragment** (`/auth/verify#token=…`), so it never reaches a server log or `Referer`. The SPA POSTs it, so mail-scanner bots that GET links can't consume it.
  - Passkeys are the encouraged primary login.
  - OAuth accounts are linked by verified email.
  - Rate limits apply per IP and per email.
- **Media**: short-lived path tokens, a cookieless media domain and R2 server-side encryption. Location is stripped from shared items by default.
- **Untrusted parsers**: ffmpeg, libvips and exiftool all parse attacker-supplied files, so workers run in locked-down containers. They get no inbound network, scoped R2 credentials, CPU/mem/time limits and a DB role that can only touch the jobs and media tables.
- **Abuse and legal**: these are table stakes for a public, user-content SaaS with share links. Even if they ship last, they ship before public launch:
  - Report button on every shared view
  - Admin takedown and account suspension
  - Share-link rate limits
  - CSAM hash scanning (e.g. Cloudflare's CSAM Scanning Tool or PhotoDNA) and the reporting process that goes with it
  - DMCA agent and process
  - ToS and privacy policy
  - GDPR-style export and delete
- **Vault (M5)**:
  - Vault items are encrypted in the browser with a per-user vault key. That key is wrapped by a passphrase via Argon2id (WASM) and/or a **WebAuthn PRF** passkey, and protected by a printed recovery key.
  - Files, client-generated thumbnails and metadata are encrypted in chunks.
  - Trade-offs: no HLS ladder (plays only if the original is browser-playable), no map, no search and no sharing in v1.
  - The schema already has `encryption` and `encrypted_meta`, so nothing needs reworking later.

---

## 7. Repo layout & tooling

```
kintube/
  backend/                 Cargo workspace
    crates/{api,app-core}  + worker, media from M1 (`app-core`, not `core`, which would shadow Rust's built-in crate)
    migrations/            sqlx migrations
  web/                     React Router v8 SPA
  edge/                    Cloudflare Worker (media gateway + share OG)
  infra/
    compose.dev.yml        postgres+postgis (multi-arch imresamu/postgis), mailpit; S3-compatible store added in M1
    deploy/                Docker images, deploy config (Kamal or compose)
  fixtures/media/          test corpus (see below)
  PLAN.md
```

- **CI**: cargo fmt/clippy/test with real Postgres and MinIO (`sqlx::test`), pnpm typecheck/lint/test, OpenAPI drift check, and a Playwright e2e test of sign-in → upload → watch.
- **Media test corpus**:
  - Video: iPhone HEVC (HDR), rotated video, vertical video, VFR, no audio, 4K, tiny, truncated or corrupt files, MKV, AVI
  - Photo: HEIC, DNG, CR3, a Live Photo pair, a 20k-px panorama, PNG with alpha
- **Observability**: `tracing` + OpenTelemetry, Sentry for errors on frontend and backend, and job queue metrics such as depth, latency and failure rate.
- **Email**: a transactional provider (Postmark, Resend or SES). Mailpit in dev.

### 7.1 Production database (self-managed)
- **Primary**: Postgres 18 + PostGIS on a dedicated Hetzner box with NVMe and ECC RAM, private network only. It is not reachable from the internet.
- **Replica**: a streaming hot standby on a second box in a different datacenter. Promotion is manual, with a written runbook, until we need automatic failover (Patroni).
- **Backups**:
  - WAL-G continuous WAL archiving plus nightly base backups, giving point-in-time recovery.
  - Backups go to a bucket at a **different provider/account** from the media (e.g. Backblaze B2), so one compromised credential or provider incident can't take both. They're encrypted client-side by WAL-G.
- **Restore drills**: a scheduled job restores the latest backup to a scratch box weekly, runs sanity queries and alerts on failure. A backup that has never been restored doesn't count.
- **Ops**:
  - `postgres_exporter` + alerts on replication lag, disk, connections, WAL archive failures and long transactions
  - Migrations run as a separate deploy step, never at app startup
  - PgBouncer only if connection counts call for it
- **Roles**: separate DB roles for `api`, `worker` (limited to jobs and media tables), `migrate` and read-only `ops`.

---

## 8. Milestones

**M0 — Foundation** ✅ done 2026-11-05
- Monorepo scaffold, dev env, CI, migrations, OpenAPI → TS client
- App shell: dark layout, sidebar, top bar, routes stubbed
- Magic-link + OTP sign-in, sessions, `/api/me`
- *Shipped*: 12 Rust tests (unit + integration on real Postgres) and 8 Playwright e2e tests (desktop + mobile), with sign-in emails read back from Mailpit. The CI workflow is written but hasn't run yet: there's no remote.
- *Carry into M1*: pick the dev S3 server. MinIO's community distribution changed in 2025; compare it with Garage and SeaweedFS before choosing. Add the `worker` and `media` crates.

**M1 — Video vertical slice** ⭐
- Resumable multipart upload to MinIO/R2 with quota reservation and an upload tray
- Postgres job queue with round-robin fairness, timeouts and retries
- Video pipeline: probe, poster, HLS ladder, scrub sprites, SSE progress
- Media tokens; dev media route plus the Cloudflare gateway Worker
- `/videos` grid and `/watch/:id` with Vidstack, quality menu and sprite scrubbing
- Delete → trash → purge job (frees R2 objects and quota)
- *Exit criterion*: a 4 GB iPhone HEVC upload survives a refresh mid-upload, becomes watchable at 360–1080p and only its owner can access it.

**M2 — The YouTube experience**
- Channels (a default one per user, plus more), playlists, autoplay next, Up-next rail
- Watch history, resume, Continue watching
- Chapters from the description, hover previews
- Search (FTS + pg_trgm), full metadata editing, bulk actions
- Passkeys and Google/Apple OAuth

**M3 — Photos**
- Image pipeline: vips, exiftool, HEIC, RAW previews, Live Photo pairing
- Timeline with buckets, virtualization and date scrubber
- Lightbox, mixed albums, map view, "On this day"

**M4 — Sharing & social**
- Unlisted links (expiry, password, download toggle, location stripping)
- Sharing with specific users, shared albums and channels with contributor roles
- Comments with timestamps, reactions, in-app notifications
- OG previews for `/s/:token`, report flow and admin moderation

**M5 — Vault**
- Client-side E2E encryption, passphrase/PRF unlock, recovery key
- Encrypted thumbnails and metadata, decrypting playback in the browser

**M6 — Launch hardening**
- Abuse and CSAM scanning, rate limits everywhere, RLS evaluation
- Failover runbook rehearsed end to end, load tests on transcode and gateway. Backups and restore drills go live with the first production database (§7.1), not left for M6.
- Admin dashboard, account export and delete, ToS and privacy policy, PWA polish

**After v1**
- Stripe billing (plans table + quota hooks already exist)
- Native app with camera-roll auto-backup
- Smart features: CLIP semantic search (pgvector), faces, Whisper captions
- AV1/HEVC renditions, which cut 4K storage and bandwidth for capable clients
- Lazy 2160p rung (encoded on first 4K playback) if 4K storage costs bite

---

## 9. Open questions

1. **Domains and trademark**: the name is **kintube**. Still needed: a trademark search (USPTO + EUIPO, classes 9/38/42) and two domains, one for the app and a cheap, separate one for user content (media).
2. **Confirm the derived limits** in §4: 50 GB per-file cap, 4 h max duration, 600 processing-minutes/month.

Resolved: name **kintube** (repo `0x36963/kintube`) · OrbStack for dev · 4K in v1 · self-managed Postgres · 20 GB default storage.
