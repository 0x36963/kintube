import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import type { components } from "./api/schema";
import { invalidateLibrary, mediaKeys, patchMedia } from "./media";

type MediaStatusEvent = components["schemas"]["MediaStatusEvent"];

/**
 * Live processing updates over server-sent events. Progress is patched straight into the
 * query cache; status changes (ready, failed, deleted) trigger a refetch, because they
 * bring new data such as posters and playback URLs. EventSource reconnects on its own.
 */
export function useMediaEvents() {
  const qc = useQueryClient();
  useEffect(() => {
    const source = new EventSource("/api/events");
    source.addEventListener("media", (e) => {
      const event = JSON.parse((e as MessageEvent<string>).data) as MediaStatusEvent;
      patchMedia(qc, event.media_id, {
        status: event.status,
        processing_progress: event.progress,
        processing_error: event.error ?? null,
      });
      if (event.status !== "processing") {
        invalidateLibrary(qc);
        void qc.invalidateQueries({ queryKey: mediaKeys.detail(event.media_id) });
      }
    });
    source.addEventListener("resync", () => invalidateLibrary(qc));
    return () => source.close();
  }, [qc]);
}
