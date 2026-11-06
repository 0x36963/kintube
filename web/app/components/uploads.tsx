import {
  Check,
  ChevronDown,
  ChevronUp,
  CircleAlert,
  CloudUpload,
  Film,
  RotateCcw,
  Upload as UploadIcon,
  X,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Link } from "react-router";

import { formatBytes } from "~/lib/format";
import {
  addFiles,
  cancel,
  clearFinished,
  dismiss,
  isActive,
  resumeWith,
  retry,
  useUploads,
  type Upload,
} from "~/lib/upload/store";

import { cx } from "./ui";

const ACCEPT = "video/*,.mkv,.mts,.m2ts,.avi,.wmv,.flv,.3gp";

/** Opens the file picker; uploads start immediately. */
export function UploadButton({ className, children }: { className?: string; children?: React.ReactNode }) {
  const input = useRef<HTMLInputElement>(null);
  return (
    <>
      <button type="button" onClick={() => input.current?.click()} className={className}>
        {children ?? (
          <>
            <UploadIcon className="size-4" />
            Upload
          </>
        )}
      </button>
      <input
        ref={input}
        type="file"
        accept={ACCEPT}
        multiple
        hidden
        data-testid="upload-input"
        onChange={(e) => {
          if (e.target.files) addFiles(e.target.files);
          e.target.value = "";
        }}
      />
    </>
  );
}

/** Drop files anywhere in the app to upload them. */
export function DropZone() {
  const [dragging, setDragging] = useState(false);
  const depth = useRef(0);

  useEffect(() => {
    const hasFiles = (e: DragEvent) => e.dataTransfer?.types.includes("Files");
    const enter = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      depth.current++;
      setDragging(true);
    };
    const leave = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      depth.current = Math.max(0, depth.current - 1);
      if (depth.current === 0) setDragging(false);
    };
    const over = (e: DragEvent) => {
      if (hasFiles(e)) e.preventDefault();
    };
    const drop = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth.current = 0;
      setDragging(false);
      const files = [...(e.dataTransfer?.files ?? [])].filter(
        (f) => f.type.startsWith("video/") || /\.(mkv|mts|m2ts|avi|wmv|flv|3gp)$/i.test(f.name),
      );
      addFiles(files);
    };
    window.addEventListener("dragenter", enter);
    window.addEventListener("dragleave", leave);
    window.addEventListener("dragover", over);
    window.addEventListener("drop", drop);
    return () => {
      window.removeEventListener("dragenter", enter);
      window.removeEventListener("dragleave", leave);
      window.removeEventListener("dragover", over);
      window.removeEventListener("drop", drop);
    };
  }, []);

  if (!dragging) return null;
  return (
    <div className="pointer-events-none fixed inset-0 z-50 grid place-items-center bg-bg/80 backdrop-blur-sm">
      <div className="flex flex-col items-center gap-4 rounded-3xl border-2 border-dashed border-accent px-16 py-12">
        <CloudUpload className="size-12 text-accent" />
        <p className="text-lg font-medium">Drop videos to upload</p>
      </div>
    </div>
  );
}

/** Bottom-right panel listing uploads, like a download shelf. */
export function UploadTray() {
  const uploads = useUploads();
  const [collapsed, setCollapsed] = useState(false);
  const active = uploads.filter(isActive);

  // Leaving the page stops uploads; warn first.
  useEffect(() => {
    if (!active.length) return;
    const warn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [active.length]);

  if (!uploads.length) return null;
  const done = uploads.filter((u) => u.status === "done").length;
  const title = active.length
    ? `Uploading ${active.length} ${active.length === 1 ? "video" : "videos"}`
    : uploads.some((u) => u.status === "interrupted")
      ? "Uploads paused"
      : `${done} ${done === 1 ? "upload" : "uploads"} complete`;

  return (
    <section
      aria-label="Uploads"
      className="fixed right-4 bottom-20 z-40 w-[min(24rem,calc(100vw-2rem))] overflow-hidden rounded-xl bg-surface-2 shadow-2xl shadow-black/60 ring-1 ring-line md:bottom-4"
    >
      <header className="flex h-12 items-center gap-2 border-b border-line pr-2 pl-4">
        <h2 className="flex-1 truncate text-sm font-medium">{title}</h2>
        <button
          type="button"
          onClick={() => setCollapsed(!collapsed)}
          aria-label={collapsed ? "Expand uploads" : "Collapse uploads"}
          className="grid size-8 place-items-center rounded-full hover:bg-surface-3"
        >
          {collapsed ? <ChevronUp className="size-4" /> : <ChevronDown className="size-4" />}
        </button>
        {!active.length && (
          <button
            type="button"
            onClick={clearFinished}
            aria-label="Close"
            className="grid size-8 place-items-center rounded-full hover:bg-surface-3"
          >
            <X className="size-4" />
          </button>
        )}
      </header>
      {!collapsed && (
        <ul className="max-h-80 divide-y divide-line overflow-y-auto">
          {uploads.map((u) => (
            <UploadRow key={u.key} upload={u} />
          ))}
        </ul>
      )}
    </section>
  );
}

function UploadRow({ upload: u }: { upload: Upload }) {
  const picker = useRef<HTMLInputElement>(null);
  const [mismatch, setMismatch] = useState<string | null>(null);
  const pct = u.size ? Math.floor((u.uploadedBytes / u.size) * 100) : 0;

  let detail: React.ReactNode;
  switch (u.status) {
    case "queued":
      detail = "Waiting…";
      break;
    case "uploading":
      detail = `${formatBytes(u.uploadedBytes)} of ${formatBytes(u.size)} · ${pct}%`;
      break;
    case "finishing":
      detail = "Finishing up…";
      break;
    case "done":
      detail = "Uploaded. Processing has started.";
      break;
    case "interrupted":
      detail = u.file ? (u.error ?? "Connection lost.") : `Paused at ${pct}%. Choose the file again to resume.`;
      break;
    case "failed":
      detail = u.error ?? "Upload failed.";
      break;
  }

  return (
    <li className="px-4 py-3" data-testid="upload-row" data-status={u.status}>
      <div className="flex items-center gap-3">
        <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-surface-3">
          {u.status === "done" ? (
            <Check className="size-4 text-emerald-400" />
          ) : u.status === "failed" ? (
            <CircleAlert className="size-4 text-danger" />
          ) : (
            <Film className="size-4 text-fg-muted" />
          )}
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm" title={u.name}>
            {u.name}
          </p>
          <p className={cx("truncate text-xs", u.status === "failed" ? "text-danger" : "text-fg-muted")}>
            {mismatch ?? detail}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {u.status === "interrupted" && !u.file && (
            <>
              <RowButton label="Resume" onClick={() => picker.current?.click()}>
                <RotateCcw className="size-4" />
              </RowButton>
              <input
                ref={picker}
                type="file"
                accept={ACCEPT}
                hidden
                data-testid="resume-input"
                onChange={(e) => {
                  const file = e.target.files?.[0];
                  if (file) setMismatch(resumeWith(u.key, file));
                  e.target.value = "";
                }}
              />
            </>
          )}
          {(u.status === "failed" || u.status === "interrupted") && u.file && (
            <RowButton label="Retry" onClick={() => retry(u.key)}>
              <RotateCcw className="size-4" />
            </RowButton>
          )}
          {u.status === "done" ? (
            <>
              {u.mediaId && (
                <Link to={`/watch/${u.mediaId}`} className="rounded-full px-2.5 py-1 text-xs font-medium text-accent hover:bg-surface-3">
                  View
                </Link>
              )}
              <RowButton label="Dismiss" onClick={() => dismiss(u.key)}>
                <X className="size-4" />
              </RowButton>
            </>
          ) : (
            <RowButton label="Cancel upload" onClick={() => void cancel(u.key)}>
              <X className="size-4" />
            </RowButton>
          )}
        </div>
      </div>
      {(u.status === "uploading" || u.status === "finishing" || u.status === "interrupted") && (
        <div className="mt-2.5 h-1 overflow-hidden rounded-full bg-surface-3" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100} aria-label={`Uploading ${u.name}`}>
          <div
            className={cx("h-full rounded-full transition-[width] duration-200", u.status === "interrupted" ? "bg-fg-subtle" : "bg-accent")}
            style={{ width: `${pct}%` }}
          />
        </div>
      )}
    </li>
  );
}

function RowButton({ label, onClick, children }: { label: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="grid size-8 place-items-center rounded-full text-fg-muted hover:bg-surface-3 hover:text-fg"
    >
      {children}
    </button>
  );
}
