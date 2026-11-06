import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CircleAlert, LoaderCircle, Pencil, Trash2 } from "lucide-react";
import { useState } from "react";
import { Link, useNavigate } from "react-router";

import { Player } from "~/components/player";
import { Button, Spinner, cx } from "~/components/ui";
import { Thumbnail } from "~/components/video-card";
import { ApiError, api, errorMessage, unwrap } from "~/lib/api/client";
import { APP_NAME } from "~/lib/brand";
import { formatBytes, formatDate, formatDuration, timeAgo } from "~/lib/format";
import {
  invalidateLibrary,
  largestPoster,
  libraryQuery,
  mediaKeys,
  mediaQuery,
  type MediaDetail,
  type MediaItem,
} from "~/lib/media";
import { queryClient } from "~/lib/session";

import type { Route } from "./+types/watch";

export async function clientLoader({ params }: Route.ClientLoaderArgs) {
  try {
    await queryClient.ensureQueryData(mediaQuery(params.id));
  } catch (err) {
    if (err instanceof ApiError && (err.status === 404 || err.status === 400)) {
      throw new Response("Not found", { status: 404 });
    }
    throw err;
  }
  return null;
}

export default function Watch({ params }: Route.ComponentProps) {
  const { data } = useQuery(mediaQuery(params.id));
  if (!data) return null;
  const { media, playback } = data;

  return (
    <div className="mx-auto grid max-w-[1800px] gap-6 pt-4 xl:grid-cols-[minmax(0,1fr)_400px]">
      <title>{`${media.title} · ${APP_NAME}`}</title>
      <div className="min-w-0">
        <div className="overflow-hidden rounded-xl bg-black">
          <div className="mx-auto aspect-video max-h-[calc(100dvh-11rem)]">
            {media.status === "ready" && playback ? (
              // Keyed by id: a fresh player per video. The URL is captured once so an
              // hourly token refresh in the cache doesn't reload the stream mid-play.
              <StablePlayer key={media.id} detail={data} />
            ) : (
              <NotPlayable media={media} />
            )}
          </div>
        </div>
        <Title media={media} />
        <Actions media={media} />
        <Details media={media} />
      </div>
      <UpNext currentId={media.id} />
    </div>
  );
}

function StablePlayer({ detail }: { detail: MediaDetail }) {
  const [source] = useState(() => ({
    hls: detail.playback!.hls_url,
    thumbnails: detail.playback!.thumbnails_url,
    poster: largestPoster(detail.media),
  }));
  return <Player hlsUrl={source.hls} thumbnailsUrl={source.thumbnails} poster={source.poster} />;
}

function NotPlayable({ media }: { media: MediaItem }) {
  if (media.status === "failed") {
    return (
      <div className="grid size-full place-items-center p-6 text-center">
        <div className="max-w-sm">
          <CircleAlert className="mx-auto size-8 text-danger" />
          <p className="mt-3 font-medium">We couldn't process this video</p>
          <p className="mt-1 text-sm text-fg-muted">{media.processing_error}</p>
        </div>
      </div>
    );
  }
  const pct = Math.round(media.processing_progress * 100);
  return (
    <div className="grid size-full place-items-center p-6 text-center">
      <div className="w-full max-w-xs">
        <LoaderCircle className="mx-auto size-8 animate-spin text-accent" />
        <p className="mt-3 font-medium">Processing your video…</p>
        <p className="mt-1 text-sm text-fg-muted">It'll play here automatically when it's ready.</p>
        <div className="mt-4 h-1.5 overflow-hidden rounded-full bg-surface-3">
          <div className="h-full bg-accent transition-[width] duration-500" style={{ width: `${pct}%` }} />
        </div>
        <p className="mt-2 text-xs text-fg-muted tabular-nums" data-testid="processing-label">
          {pct > 0 ? `${pct}%` : "Preparing…"}
        </p>
      </div>
    </div>
  );
}

function useUpdateMedia(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (body: { title?: string; description?: string }) =>
      unwrap(await api.PATCH("/api/media/{id}", { params: { path: { id } }, body })),
    onSuccess: (item) => {
      qc.setQueryData<MediaDetail>(mediaKeys.detail(id), (d) => (d ? { ...d, media: item } : d));
      void qc.invalidateQueries({ queryKey: mediaKeys.library });
    },
  });
}

function Title({ media }: { media: MediaItem }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(media.title);
  const update = useUpdateMedia(media.id);

  if (!editing) {
    return (
      <div className="group mt-4 flex items-start gap-2">
        <h1 className="text-xl leading-snug font-semibold break-words">{media.title}</h1>
        <button
          type="button"
          onClick={() => {
            setDraft(media.title);
            setEditing(true);
          }}
          aria-label="Edit title"
          className="mt-0.5 grid size-7 shrink-0 place-items-center rounded-full text-fg-muted opacity-0 group-hover:opacity-100 hover:bg-surface-2 focus-visible:opacity-100"
        >
          <Pencil className="size-3.5" />
        </button>
      </div>
    );
  }
  return (
    <form
      className="mt-4"
      onSubmit={(e) => {
        e.preventDefault();
        update.mutate({ title: draft }, { onSuccess: () => setEditing(false) });
      }}
    >
      <input
        autoFocus
        aria-label="Title"
        value={draft}
        maxLength={200}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && setEditing(false)}
        className="w-full rounded-lg bg-surface-2 px-3 py-2 text-xl font-semibold ring-1 ring-line outline-none focus:ring-2 focus:ring-accent"
      />
      {update.isError && <p className="mt-2 text-sm text-danger">{errorMessage(update.error)}</p>}
      <div className="mt-2 flex gap-2">
        <Button type="submit" disabled={update.isPending || !draft.trim()} className="h-9">
          Save
        </Button>
        <Button type="button" variant="ghost" className="h-9" onClick={() => setEditing(false)}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

function Actions({ media }: { media: MediaItem }) {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const trash = useMutation({
    mutationFn: async () => unwrap(await api.DELETE("/api/media/{id}", { params: { path: { id: media.id } } })),
    onSuccess: () => {
      invalidateLibrary(qc);
      navigate("/videos", { replace: true });
    },
  });
  return (
    <div className="mt-3 flex flex-wrap items-center gap-2">
      <Button variant="secondary" className="h-9 px-4" onClick={() => trash.mutate()} disabled={trash.isPending}>
        <Trash2 className="size-4" />
        Delete
      </Button>
      {trash.isError && <p className="text-sm text-danger">{errorMessage(trash.error)}</p>}
    </div>
  );
}

function Details({ media }: { media: MediaItem }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(media.description);
  const update = useUpdateMedia(media.id);
  const facts = [
    media.taken_at && `Recorded ${formatDate(media.taken_at, true)}`,
    `Uploaded ${timeAgo(media.created_at)}`,
    media.width && media.height && `${media.width}×${media.height}`,
    media.hdr && "HDR",
    media.duration_ms != null && formatDuration(media.duration_ms),
    formatBytes(media.bytes),
  ].filter(Boolean);

  return (
    <section className="mt-4 rounded-xl bg-surface p-4 text-sm">
      <p className="font-medium">{facts.join(" · ")}</p>
      {editing ? (
        <form
          className="mt-3"
          onSubmit={(e) => {
            e.preventDefault();
            update.mutate({ description: draft }, { onSuccess: () => setEditing(false) });
          }}
        >
          <textarea
            autoFocus
            aria-label="Description"
            value={draft}
            maxLength={5000}
            rows={4}
            onChange={(e) => setDraft(e.target.value)}
            className="w-full resize-y rounded-lg bg-surface-2 px-3 py-2 ring-1 ring-line outline-none focus:ring-2 focus:ring-accent"
          />
          <div className="mt-2 flex gap-2">
            <Button type="submit" disabled={update.isPending} className="h-9">
              Save
            </Button>
            <Button type="button" variant="ghost" className="h-9" onClick={() => setEditing(false)}>
              Cancel
            </Button>
          </div>
        </form>
      ) : (
        <button
          type="button"
          onClick={() => {
            setDraft(media.description);
            setEditing(true);
          }}
          className={cx(
            "mt-2 block w-full rounded text-left whitespace-pre-wrap",
            media.description ? "text-fg" : "text-fg-subtle hover:text-fg-muted",
          )}
        >
          {media.description || "Add a description"}
        </button>
      )}
      <p className="mt-3 truncate text-xs text-fg-subtle" title={media.original_filename}>
        {media.original_filename}
      </p>
    </section>
  );
}

function UpNext({ currentId }: { currentId: string }) {
  const { data } = useInfiniteQuery(libraryQuery);
  const items = (data?.pages[0]?.items ?? []).filter((m) => m.id !== currentId && m.status === "ready").slice(0, 12);
  if (!data) {
    return (
      <aside className="hidden xl:block">
        <Spinner className="size-5" />
      </aside>
    );
  }
  if (!items.length) return <aside />;
  return (
    <aside aria-label="More videos">
      <h2 className="mb-3 text-sm font-semibold xl:sr-only">More videos</h2>
      <ul className="space-y-3">
        {items.map((item) => (
          <li key={item.id}>
            <Link to={`/watch/${item.id}`} className="group flex gap-3 rounded-lg">
              <Thumbnail item={item} sizes="168px" className="w-40 shrink-0 rounded-lg" />
              <div className="min-w-0 py-0.5">
                <h3 className="line-clamp-2 text-sm leading-snug font-medium">{item.title}</h3>
                <p className="mt-1 text-xs text-fg-muted">
                  {item.taken_at ? formatDate(item.taken_at) : timeAgo(item.created_at)}
                </p>
              </div>
            </Link>
          </li>
        ))}
      </ul>
    </aside>
  );
}
