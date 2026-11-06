import { useInfiniteQuery } from "@tanstack/react-query";
import { CircleAlert, LoaderCircle } from "lucide-react";
import { useEffect, useRef } from "react";
import { Link } from "react-router";

import { formatDate, formatDuration, timeAgo } from "~/lib/format";
import { isPortrait, libraryQuery, posterSrcSet, qualityLabel, type MediaItem } from "~/lib/media";

import { cx } from "./ui";

const GRID_SIZES = "(min-width: 1536px) 25vw, (min-width: 1024px) 33vw, (min-width: 640px) 50vw, 100vw";

/** The 16:9 picture for an item: poster, or processing / failed state. */
export function Thumbnail({ item, sizes = GRID_SIZES, className }: { item: MediaItem; sizes?: string; className?: string }) {
  const srcSet = posterSrcSet(item);
  const portrait = isPortrait(item);
  return (
    <div className={cx("relative aspect-video overflow-hidden rounded-xl bg-surface", className)}>
      {srcSet && (
        <>
          {/* Portrait video: blurred fill behind a contained poster, instead of a hard crop. */}
          {portrait && (
            <img
              src={item.posters[0]?.url}
              alt=""
              aria-hidden="true"
              className="absolute inset-0 size-full scale-110 object-cover opacity-60 blur-xl"
            />
          )}
          <img
            srcSet={srcSet}
            sizes={sizes}
            alt=""
            loading="lazy"
            decoding="async"
            className={cx(
              "relative size-full transition-transform duration-300 group-hover:scale-[1.03]",
              portrait ? "object-contain" : "object-cover",
            )}
          />
        </>
      )}
      {item.status === "ready" && item.duration_ms != null && (
        <span className="absolute right-1.5 bottom-1.5 rounded bg-black/80 px-1 py-0.5 text-xs font-medium tabular-nums">
          {formatDuration(item.duration_ms)}
        </span>
      )}
      {item.status === "processing" && <ProcessingOverlay progress={item.processing_progress} />}
      {item.status === "failed" && (
        <div className="absolute inset-0 grid place-items-center bg-surface p-4 text-center">
          <div>
            <CircleAlert className="mx-auto size-6 text-danger" />
            <p className="mt-2 text-xs text-fg-muted">Couldn't process</p>
          </div>
        </div>
      )}
    </div>
  );
}

function ProcessingOverlay({ progress }: { progress: number }) {
  const pct = Math.round(progress * 100);
  return (
    <div className="absolute inset-0 grid place-items-center bg-surface/90">
      <div className="text-center">
        <LoaderCircle className="mx-auto size-6 animate-spin text-accent" />
        <p className="mt-2 text-xs font-medium text-fg-muted tabular-nums" data-testid="processing-label">
          {pct > 0 ? `Processing ${pct}%` : "Preparing…"}
        </p>
      </div>
      <div className="absolute inset-x-0 bottom-0 h-1 bg-surface-3">
        <div className="h-full bg-accent transition-[width] duration-500" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

export function VideoCard({ item }: { item: MediaItem }) {
  const quality = qualityLabel(item);
  return (
    <Link to={`/watch/${item.id}`} className="group block rounded-xl" data-testid="video-card" data-status={item.status}>
      <Thumbnail item={item} />
      <div className="mt-3 pr-2">
        <h3 className="line-clamp-2 text-sm leading-snug font-medium">{item.title}</h3>
        <p className="mt-1 flex flex-wrap items-center gap-x-1.5 text-xs text-fg-muted">
          <span>{item.taken_at ? formatDate(item.taken_at) : `Uploaded ${timeAgo(item.created_at)}`}</span>
          {quality && (
            <span className="rounded bg-surface-2 px-1 text-[10px] font-semibold tracking-wide text-fg-muted ring-1 ring-line">
              {quality}
            </span>
          )}
          {item.hdr && (
            <span className="rounded bg-surface-2 px-1 text-[10px] font-semibold tracking-wide text-fg-muted ring-1 ring-line">
              HDR
            </span>
          )}
        </p>
      </div>
    </Link>
  );
}

export function VideoGridSkeleton({ count = 8 }: { count?: number }) {
  return (
    <>
      {Array.from({ length: count }, (_, i) => (
        <div key={i} aria-hidden="true">
          <div className="skeleton aspect-video rounded-xl" />
          <div className="mt-3 space-y-2">
            <div className="skeleton h-3.5 w-11/12 rounded" />
            <div className="skeleton h-3 w-1/2 rounded" />
          </div>
        </div>
      ))}
    </>
  );
}

export const gridClass =
  "grid grid-cols-1 gap-x-4 gap-y-8 sm:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-4";

/** The library as an infinitely scrolling grid. `limit` caps it (e.g. on Home). */
export function VideoGrid({ limit, empty }: { limit?: number; empty: React.ReactNode }) {
  const query = useInfiniteQuery(libraryQuery);
  const sentinel = useRef<HTMLDivElement>(null);
  const items = query.data?.pages.flatMap((p) => p.items) ?? [];
  const shown = limit ? items.slice(0, limit) : items;
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = query;

  useEffect(() => {
    if (limit || !sentinel.current || !hasNextPage) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting && !isFetchingNextPage) void fetchNextPage();
      },
      { rootMargin: "800px" },
    );
    observer.observe(sentinel.current);
    return () => observer.disconnect();
  }, [limit, hasNextPage, isFetchingNextPage, fetchNextPage]);

  if (query.isPending) {
    return (
      <div className={gridClass}>
        <VideoGridSkeleton />
      </div>
    );
  }
  if (query.isError) {
    return <p className="py-16 text-center text-sm text-fg-muted">We couldn't load your videos. Try reloading the page.</p>;
  }
  if (!items.length) return <>{empty}</>;
  return (
    <>
      <div className={gridClass}>
        {shown.map((item) => (
          <VideoCard key={item.id} item={item} />
        ))}
        {isFetchingNextPage && <VideoGridSkeleton count={4} />}
      </div>
      <div ref={sentinel} aria-hidden="true" />
    </>
  );
}
