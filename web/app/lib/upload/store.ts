/**
 * Upload manager: a queue of uploads shared by the upload button, drop zone and tray.
 *
 * In-progress uploads are remembered in localStorage. After a refresh they show up as
 * "interrupted"; picking the same file again resumes from the parts already stored.
 * (Browsers don't let a page reopen a file by itself, so the user has to pick it.)
 */
import { useSyncExternalStore } from "react";

import { ApiError, api, errorMessage, unwrap } from "../api/client";
import { invalidateLibrary } from "../media";
import { queryClient } from "../session";
import { uploadParts, type UploadInfo } from "./uploader";

export type UploadStatus =
  | "queued"
  | "uploading"
  | "finishing"
  | "done"
  | "failed"
  /** Stopped mid-way (refresh, network); resumable. */
  | "interrupted";

export interface Upload {
  key: string;
  name: string;
  size: number;
  status: UploadStatus;
  uploadedBytes: number;
  mediaId?: string;
  error?: string;
  /** Missing after a refresh until the user picks the file again. */
  file?: File;
}

const STORAGE_KEY = "kintube.uploads.v1";
const MAX_ACTIVE = 2;

let uploads: Upload[] = [];
const listeners = new Set<() => void>();
const controllers = new Map<string, AbortController>();

function set(next: Upload[]) {
  uploads = next;
  persist();
  listeners.forEach((l) => l());
}

function update(key: string, patch: Partial<Upload>) {
  set(uploads.map((u) => (u.key === key ? { ...u, ...patch } : u)));
}

function persist() {
  const resumable = uploads
    .filter((u) => u.mediaId && !["done", "failed"].includes(u.status))
    .map(({ key, name, size, mediaId, uploadedBytes }) => ({ key, name, size, mediaId, uploadedBytes }));
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(resumable));
  } catch {
    // Storage full or disabled: uploads still work, they just can't resume after a refresh.
  }
}

let restored = false;
function restore() {
  if (restored || typeof window === "undefined") return;
  restored = true;
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]") as Omit<Upload, "status">[];
    uploads = saved.map((u) => ({ ...u, status: "interrupted" as const }));
  } catch {
    uploads = [];
  }
}

export function useUploads(): Upload[] {
  restore();
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => uploads,
    () => uploads,
  );
}

export function isActive(u: Upload) {
  return u.status === "queued" || u.status === "uploading" || u.status === "finishing";
}

export function addFiles(files: Iterable<File>) {
  restore();
  const added = [...files].map<Upload>((file) => ({
    key: crypto.randomUUID(),
    name: file.name,
    size: file.size,
    status: "queued",
    uploadedBytes: 0,
    file,
  }));
  if (added.length) {
    set([...uploads, ...added]);
    pump();
  }
}

/** Resume an interrupted upload with the file the user picked again. */
export function resumeWith(key: string, file: File): string | null {
  const upload = uploads.find((u) => u.key === key);
  if (!upload) return null;
  if (file.name !== upload.name || file.size !== upload.size) {
    return `That's not the same file. Choose "${upload.name}" to resume.`;
  }
  update(key, { file, status: "queued", error: undefined });
  pump();
  return null;
}

/** Retry after a failure (keeps already-stored parts). */
export function retry(key: string) {
  const upload = uploads.find((u) => u.key === key);
  if (upload?.file) {
    update(key, { status: "queued", error: undefined });
    pump();
  }
}

export async function cancel(key: string) {
  const upload = uploads.find((u) => u.key === key);
  controllers.get(key)?.abort();
  set(uploads.filter((u) => u.key !== key));
  if (upload?.mediaId && upload.status !== "done") {
    // Free the storage reservation. Best effort: stale uploads are also swept server-side.
    await api.DELETE("/api/uploads/{id}", { params: { path: { id: upload.mediaId } } }).catch(() => {});
  }
}

export function dismiss(key: string) {
  set(uploads.filter((u) => u.key !== key || isActive(u)));
}

export function clearFinished() {
  set(uploads.filter((u) => u.status !== "done"));
}

function pump() {
  const active = uploads.filter((u) => u.status === "uploading" || u.status === "finishing").length;
  const next = uploads.filter((u) => u.status === "queued").slice(0, Math.max(0, MAX_ACTIVE - active));
  next.forEach((u) => void run(u.key));
}

async function run(key: string) {
  const initial = uploads.find((u) => u.key === key);
  if (!initial?.file) return;
  const file = initial.file;
  const controller = new AbortController();
  controllers.set(key, controller);
  update(key, { status: "uploading" });

  try {
    let info: UploadInfo;
    if (initial.mediaId) {
      info = unwrap(await api.GET("/api/uploads/{id}", { params: { path: { id: initial.mediaId } } }));
    } else {
      info = unwrap(
        await api.POST("/api/uploads", { body: { filename: file.name, size: file.size, mime_type: file.type } }),
      );
      update(key, { mediaId: info.media_id });
      invalidateLibrary(queryClient); // usage now includes the reservation
    }

    let last = 0;
    await uploadParts({
      file,
      info,
      signal: controller.signal,
      onProgress: (bytes) => {
        // Re-render at most ~10×/s.
        const now = performance.now();
        if (now - last > 100 || bytes === file.size) {
          last = now;
          update(key, { uploadedBytes: bytes });
        }
      },
    });

    update(key, { status: "finishing", uploadedBytes: file.size });
    unwrap(await api.POST("/api/uploads/{id}/complete", { params: { path: { id: info.media_id } } }));
    update(key, { status: "done", file: undefined });
    invalidateLibrary(queryClient);
  } catch (err) {
    if (controller.signal.aborted) return; // canceled; already removed
    if (err instanceof ApiError && (err.status === 404 || err.code === "invalid_state")) {
      // The upload expired server-side or already finished elsewhere.
      update(key, { status: "failed", mediaId: undefined, error: "This upload expired. Upload the file again." });
    } else if (err instanceof ApiError) {
      // Keeps mediaId (if any), so Retry resumes instead of starting over.
      update(key, { status: "failed", error: err.message });
    } else {
      update(key, { status: "interrupted", error: errorMessage(err) });
    }
  } finally {
    controllers.delete(key);
    pump();
  }
}

