/**
 * kintube media gateway (Cloudflare Worker).
 *
 * Serves `/{token}/m/{media_id}/{path}` from R2 on the cookieless media domain. The token
 * (issued by the API) authorizes everything under one media item until it expires; HLS
 * playlists use relative URLs, so segments inherit it. Mirrors the API's dev-only route
 * (backend/crates/api/src/routes/media_files.rs).
 */
import { verifyToken } from "./token";

export interface Env {
  MEDIA: R2Bucket;
  /** Same value as the API's MEDIA_TOKEN_SECRET. Set with `wrangler secret put`. */
  MEDIA_TOKEN_SECRET: string;
}

const PATH = /^\/([^/]+)\/m\/([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\/(.+)$/;

const CONTENT_TYPES: Record<string, string> = {
  m3u8: "application/vnd.apple.mpegurl",
  m4s: "video/iso.segment",
  mp4: "video/mp4",
  webp: "image/webp",
  jpg: "image/jpeg",
  vtt: "text/vtt; charset=utf-8",
};

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    return handle(request, env, Math.floor(Date.now() / 1000));
  },
} satisfies ExportedHandler<Env>;

export async function handle(request: Request, env: Env, nowSecs: number): Promise<Response> {
  if (request.method === "OPTIONS") return preflight();
  if (request.method !== "GET" && request.method !== "HEAD") return text(405, "method not allowed");

  const match = PATH.exec(new URL(request.url).pathname);
  if (!match) return text(404, "not found");
  const [, token, rawId, rawPath] = match as unknown as [string, string, string, string];
  const mediaId = rawId.toLowerCase();
  if (!(await verifyToken(env.MEDIA_TOKEN_SECRET, token, mediaId, nowSecs))) {
    return text(403, "invalid or expired media token");
  }

  let path: string;
  try {
    path = decodeURIComponent(rawPath);
  } catch {
    return text(404, "not found");
  }
  // Derived files only, and no way out of the item's folder.
  const extension = path.split(".").pop() ?? "";
  const safe =
    path !== "original" &&
    /^[A-Za-z0-9/._-]+$/.test(path) &&
    !path.split("/").some((seg) => seg === "" || seg === "." || seg === "..") &&
    extension in CONTENT_TYPES;
  if (!safe) return text(404, "not found");

  const object = await env.MEDIA.get(`m/${mediaId}/${path}`, {
    range: request.headers,
    onlyIf: request.headers,
  });
  if (!object) return text(404, "not found");

  const headers = new Headers({
    "Content-Type": CONTENT_TYPES[extension]!,
    "Accept-Ranges": "bytes",
    // Files never change once written; the token in the URL rotates hourly.
    "Cache-Control": "private, max-age=86400, immutable",
    ETag: object.httpEtag,
    ...CORS,
    "X-Content-Type-Options": "nosniff",
    "Cross-Origin-Resource-Policy": "cross-origin",
  });

  if (!("body" in object)) {
    // A conditional request (If-None-Match…) matched: nothing to send.
    return new Response(null, { status: 304, headers });
  }
  const range = object.range as { offset?: number; length?: number; suffix?: number } | undefined;
  if (range && request.headers.has("range")) {
    const start = range.suffix != null ? object.size - range.suffix : (range.offset ?? 0);
    const length = range.suffix ?? range.length ?? object.size - start;
    headers.set("Content-Range", `bytes ${start}-${start + length - 1}/${object.size}`);
    headers.set("Content-Length", String(length));
    return new Response(request.method === "HEAD" ? null : object.body, { status: 206, headers });
  }
  headers.set("Content-Length", String(object.size));
  return new Response(request.method === "HEAD" ? null : object.body, { status: 200, headers });
}

const CORS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Expose-Headers": "Content-Length, Content-Range",
};

function preflight(): Response {
  return new Response(null, {
    status: 204,
    headers: {
      ...CORS,
      "Access-Control-Allow-Methods": "GET, HEAD, OPTIONS",
      "Access-Control-Allow-Headers": "Range",
      "Access-Control-Max-Age": "86400",
    },
  });
}

function text(status: number, message: string): Response {
  return new Response(message, { status, headers: { ...CORS, "Content-Type": "text/plain; charset=utf-8" } });
}
