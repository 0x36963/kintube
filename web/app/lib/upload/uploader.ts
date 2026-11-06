/**
 * Multipart upload of one file straight to object storage, via presigned part URLs from
 * our API. Resumable: pass the part numbers already stored and only the rest are sent.
 */
import { api, unwrap } from "../api/client";
import type { components } from "../api/schema";

export type UploadInfo = components["schemas"]["UploadInfo"];

const PARALLEL_PARTS = 4;
const PRESIGN_BATCH = 20;
const MAX_ATTEMPTS = 6;

export class PartUploadError extends Error {
  constructor(readonly status: number) {
    super(status ? `Storage rejected a part (HTTP ${status})` : "Network error while uploading");
  }
}

export async function uploadParts(opts: {
  file: Blob;
  info: UploadInfo;
  signal: AbortSignal;
  /** Total bytes stored so far, including earlier sessions. */
  onProgress: (uploadedBytes: number) => void;
}): Promise<void> {
  const { file, info, signal, onProgress } = opts;
  const partSize = info.part_size;
  const sizeOf = (n: number) => Math.min(partSize, file.size - (n - 1) * partSize);

  const stored = new Set(info.uploaded_parts);
  const queue: number[] = [];
  for (let n = 1; n <= info.part_count; n++) if (!stored.has(n)) queue.push(n);

  let doneBytes = [...stored].reduce((sum, n) => sum + sizeOf(n), 0);
  const inFlight = new Map<number, number>();
  const report = () => onProgress(doneBytes + [...inFlight.values()].reduce((a, b) => a + b, 0));
  report();

  // Presign lazily, in batches, so a huge file doesn't need thousands of URLs up front
  // (and none expire while they wait).
  const urls = new Map<number, Promise<string>>();
  const urlFor = (n: number): Promise<string> => {
    if (!urls.has(n)) {
      const batch = [n, ...queue.filter((m) => !urls.has(m))].slice(0, PRESIGN_BATCH);
      const request = api
        .POST("/api/uploads/{id}/parts", { params: { path: { id: info.media_id } }, body: { part_numbers: batch } })
        .then(unwrap)
        .then((res) => new Map(res.parts.map((p) => [p.part_number, p.url])));
      for (const m of batch) urls.set(m, request.then((map) => map.get(m)!));
      request.catch(() => batch.forEach((m) => urls.delete(m)));
    }
    return urls.get(n)!;
  };

  const uploadOne = async (n: number) => {
    const blob = file.slice((n - 1) * partSize, (n - 1) * partSize + sizeOf(n));
    for (let attempt = 1; ; attempt++) {
      try {
        const url = await urlFor(n);
        await putBlob(url, blob, signal, (loaded) => {
          inFlight.set(n, loaded);
          report();
        });
        inFlight.delete(n);
        doneBytes += blob.size;
        report();
        return;
      } catch (err) {
        inFlight.delete(n);
        if (signal.aborted) throw err;
        if (attempt >= MAX_ATTEMPTS) throw err;
        // An expired or rejected URL gets re-signed on the next attempt.
        if (err instanceof PartUploadError && err.status === 403) urls.delete(n);
        await waitBeforeRetry(attempt, signal);
      }
    }
  };

  const workers = Array.from({ length: Math.min(PARALLEL_PARTS, queue.length) }, async () => {
    while (queue.length) {
      signal.throwIfAborted();
      await uploadOne(queue.shift()!);
    }
  });
  await Promise.all(workers);
}

/** XHR rather than fetch: fetch can't report upload progress. */
function putBlob(url: string, blob: Blob, signal: AbortSignal, onProgress: (loaded: number) => void): Promise<void> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("PUT", url);
    xhr.upload.onprogress = (e) => onProgress(e.loaded);
    xhr.onload = () => (xhr.status >= 200 && xhr.status < 300 ? resolve() : reject(new PartUploadError(xhr.status)));
    xhr.onerror = () => reject(new PartUploadError(0));
    xhr.onabort = () => reject(new DOMException("Upload canceled", "AbortError"));
    signal.addEventListener("abort", () => xhr.abort(), { once: true });
    xhr.send(blob);
  });
}

/** Exponential backoff (1 s, 2 s, 4 s…), and if offline, wait to be back online. */
async function waitBeforeRetry(attempt: number, signal: AbortSignal) {
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(resolve, 1000 * 2 ** (attempt - 1));
    signal.addEventListener("abort", () => {
      clearTimeout(timer);
      reject(new DOMException("Upload canceled", "AbortError"));
    }, { once: true });
  });
  if (!navigator.onLine) {
    await new Promise<void>((resolve) => window.addEventListener("online", () => resolve(), { once: true }));
  }
}
