import { infiniteQueryOptions, queryOptions, type InfiniteData, type QueryClient } from "@tanstack/react-query";

import { api, unwrap } from "./api/client";
import type { components } from "./api/schema";

export type MediaItem = components["schemas"]["MediaItem"];
export type MediaDetail = components["schemas"]["MediaDetail"];
export type MediaPage = components["schemas"]["MediaPage"];
export type Usage = components["schemas"]["Usage"];

export const mediaKeys = {
  all: ["media"] as const,
  library: ["media", "library"] as const,
  detail: (id: string) => ["media", "detail", id] as const,
  trash: ["media", "trash"] as const,
  usage: ["usage"] as const,
};

export const libraryQuery = infiniteQueryOptions({
  queryKey: mediaKeys.library,
  queryFn: async ({ pageParam }) =>
    unwrap(await api.GET("/api/media", { params: { query: { cursor: pageParam, limit: 24 } } })),
  initialPageParam: undefined as string | undefined,
  getNextPageParam: (last) => last.next_cursor ?? undefined,
});

export const mediaQuery = (id: string) =>
  queryOptions({
    queryKey: mediaKeys.detail(id),
    queryFn: async () => unwrap(await api.GET("/api/media/{id}", { params: { path: { id } } })),
  });

export const trashQuery = queryOptions({
  queryKey: mediaKeys.trash,
  queryFn: async () => unwrap(await api.GET("/api/trash")),
});

export const usageQuery = queryOptions({
  queryKey: mediaKeys.usage,
  queryFn: async () => unwrap(await api.GET("/api/me/usage")),
});

/** After anything that changes the library or quota. */
export function invalidateLibrary(qc: QueryClient) {
  void qc.invalidateQueries({ queryKey: mediaKeys.all });
  void qc.invalidateQueries({ queryKey: mediaKeys.usage });
}

/** Applies a live status update to every cached copy of an item. */
export function patchMedia(qc: QueryClient, id: string, patch: Partial<MediaItem>) {
  qc.setQueryData<MediaDetail>(mediaKeys.detail(id), (d) => (d ? { ...d, media: { ...d.media, ...patch } } : d));
  qc.setQueryData<InfiniteData<MediaPage>>(mediaKeys.library, (data) =>
    data
      ? {
          ...data,
          pages: data.pages.map((page) => ({
            ...page,
            items: page.items.map((item) => (item.id === id ? { ...item, ...patch } : item)),
          })),
        }
      : data,
  );
}

/** `srcSet` for an item's posters (smallest first). */
export function posterSrcSet(item: MediaItem): string | undefined {
  if (!item.posters.length) return undefined;
  return item.posters.map((p) => `${p.url} ${Math.max(p.width, p.height)}w`).join(", ");
}

export function largestPoster(item: MediaItem): string | undefined {
  return item.posters.at(-1)?.url;
}

export function isPortrait(item: MediaItem): boolean {
  return !!item.width && !!item.height && item.height > item.width;
}

export function qualityLabel(item: MediaItem): string | null {
  if (!item.width || !item.height) return null;
  const short = Math.min(item.width, item.height);
  if (short >= 2100) return "4K";
  if (short >= 1400) return "1440p";
  if (short >= 1060) return "HD";
  return null;
}
