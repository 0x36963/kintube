-- Media library, quotas, and the background job queue (M1).

-- Per-user limits. Plain columns for now; paid plans will set them (PLAN.md §4).
alter table users
    add column storage_limit_bytes      bigint not null default 21474836480, -- 20 GiB
    add column storage_used_bytes       bigint not null default 0,           -- originals of uploaded media
    add column storage_reserved_bytes   bigint not null default 0,           -- uploads in progress
    add column processing_minutes_limit int    not null default 600,         -- weighted minutes per month
    add constraint users_storage_nonnegative
        check (storage_used_bytes >= 0 and storage_reserved_bytes >= 0);

-- Weighted processing seconds per user per calendar month (UTC):
-- source seconds × 1 (≤1080p), × 2 (1440p), × 4 (2160p).
create table processing_usage (
    user_id          uuid not null references users (id) on delete cascade,
    month            date not null,
    weighted_seconds bigint not null default 0,
    primary key (user_id, month)
);

create table media (
    id                  uuid primary key,
    owner_id            uuid not null references users (id) on delete cascade,
    kind                text not null check (kind in ('video', 'photo')),
    status              text not null check (status in ('uploading', 'processing', 'ready', 'failed')),
    title               text not null,
    description         text not null default '',
    original_filename   text not null,
    mime                text not null,
    bytes               bigint not null check (bytes > 0),
    -- Set while uploading.
    upload_id           text,
    part_size           bigint,
    -- Filled in by processing.
    width               int,          -- display size, after rotation
    height              int,
    duration_ms         bigint,
    video_codec         text,
    audio_codec         text,
    hdr                 boolean,
    taken_at            timestamptz,  -- from container metadata, when present
    processing_progress real not null default 0,
    processing_error    text,          -- user-facing reason when status = 'failed'
    created_at          timestamptz not null default now(),
    updated_at          timestamptz not null default now(),
    ready_at            timestamptz,
    trashed_at          timestamptz
);
create index media_library on media (owner_id, created_at desc, id desc) where trashed_at is null;
create index media_trash on media (owner_id, trashed_at desc) where trashed_at is not null;
create index media_stale_uploads on media (created_at) where status = 'uploading';

-- Derived files (all stored under m/{media_id}/ in object storage).
create table media_assets (
    media_id uuid not null references media (id) on delete cascade,
    kind     text not null check (kind in ('hls', 'poster', 'sprites')),
    path     text not null,                 -- relative to m/{media_id}/
    bytes    bigint not null default 0,     -- total size of all files of this asset
    meta     jsonb not null default '{}',
    primary key (media_id, kind)
);

-- Background jobs. Claimed with FOR UPDATE SKIP LOCKED; see app_core::jobs.
create table jobs (
    id           uuid primary key,
    kind         text not null,
    owner_id     uuid references users (id) on delete cascade, -- for per-user fairness; null = system
    payload      jsonb not null default '{}',
    status       text not null default 'queued' check (status in ('queued', 'running', 'done', 'failed')),
    attempts     int not null default 0,
    max_attempts int not null default 3,
    run_at       timestamptz not null default now(),
    locked_by    text,
    locked_until timestamptz,
    last_error   text,
    created_at   timestamptz not null default now(),
    updated_at   timestamptz not null default now()
);
create index jobs_queued on jobs (run_at) where status = 'queued';
create index jobs_running on jobs (owner_id) where status = 'running';
create index jobs_expired_leases on jobs (locked_until) where status = 'running';
