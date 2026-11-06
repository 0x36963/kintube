import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RotateCcw, Trash2 } from "lucide-react";

import { Button, EmptyState, Spinner } from "~/components/ui";
import { Thumbnail } from "~/components/video-card";
import { api, errorMessage, unwrap } from "~/lib/api/client";
import { APP_NAME } from "~/lib/brand";
import { formatBytes, timeAgo } from "~/lib/format";
import { invalidateLibrary, trashQuery, type MediaItem } from "~/lib/media";

export function meta() {
  return [{ title: `Trash · ${APP_NAME}` }];
}

export default function Trash() {
  const { data, isPending } = useQuery(trashQuery);

  return (
    <div className="mx-auto max-w-4xl pt-4">
      <h1 className="text-2xl font-semibold tracking-tight">Trash</h1>
      {data && data.items.length > 0 && (
        <p className="mt-1 text-sm text-fg-muted">
          Videos in the trash are deleted for good after {data.retention_days} days.
        </p>
      )}
      {isPending ? (
        <div className="py-16 text-center">
          <Spinner />
        </div>
      ) : !data?.items.length ? (
        <EmptyState icon={Trash2} title="Trash is empty">
          Deleted videos stay here for {data?.retention_days ?? 30} days, so you can change your mind.
        </EmptyState>
      ) : (
        <ul className="mt-6 divide-y divide-line">
          {data.items.map((item) => (
            <TrashRow key={item.id} item={item} />
          ))}
        </ul>
      )}
    </div>
  );
}

function TrashRow({ item }: { item: MediaItem }) {
  const qc = useQueryClient();
  const onSuccess = () => invalidateLibrary(qc);
  const restore = useMutation({
    mutationFn: async () => unwrap(await api.POST("/api/media/{id}/restore", { params: { path: { id: item.id } } })),
    onSuccess,
  });
  const purge = useMutation({
    mutationFn: async () => unwrap(await api.DELETE("/api/trash/{id}", { params: { path: { id: item.id } } })),
    onSuccess,
  });
  const error = restore.error ?? purge.error;

  return (
    <li className="flex items-center gap-4 py-3" data-testid="trash-row">
      <Thumbnail item={item} sizes="160px" className="w-36 shrink-0 rounded-lg" />
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium">{item.title}</p>
        <p className="text-sm text-fg-muted">
          Deleted {item.trashed_at ? timeAgo(item.trashed_at) : ""} · {formatBytes(item.bytes)}
        </p>
        {error && <p className="text-sm text-danger">{errorMessage(error)}</p>}
      </div>
      <div className="flex shrink-0 gap-2">
        <Button variant="secondary" className="h-9 px-4" onClick={() => restore.mutate()} disabled={restore.isPending}>
          <RotateCcw className="size-4" />
          Restore
        </Button>
        <Button
          variant="ghost"
          className="h-9 px-4 hover:text-danger"
          disabled={purge.isPending}
          onClick={() => {
            if (window.confirm(`Delete "${item.title}" for good? This can't be undone.`)) purge.mutate();
          }}
        >
          Delete forever
        </Button>
      </div>
    </li>
  );
}
