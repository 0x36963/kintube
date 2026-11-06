import { describe, expect, it } from "vitest";

import vectors from "../../fixtures/media-token-vectors.json";
import { type Env, handle } from "../src/index";
import { signToken, verifyToken } from "../src/token";

describe("tokens", () => {
  it("match the shared test vectors (generated independently, also checked by Rust)", async () => {
    for (const c of vectors.cases) {
      expect(await signToken(vectors.secret, c.media_id, c.expires)).toBe(c.token);
      expect(await verifyToken(vectors.secret, c.token, c.media_id, c.expires - 1)).toBe(true);
    }
  });

  it("reject expired, tampered, foreign and malformed tokens", async () => {
    const c = vectors.cases[0]!;
    const [exp, sig] = c.token.split(".") as [string, string];
    const verify = (token: string, id = c.media_id, now = c.expires - 1) => verifyToken(vectors.secret, token, id, now);
    expect(await verify(c.token, c.media_id, c.expires)).toBe(false); // expired
    expect(await verify(`${Number(exp) + 3600}.${sig}`)).toBe(false); // extended
    expect(await verify(c.token, vectors.cases[2]!.media_id)).toBe(false); // other item
    expect(await verify(`${exp}.AAAAAAAAAAAAAAAAAAAAAA`)).toBe(false);
    for (const junk of ["", ".", "abc", `${exp}.`, `-1.${sig}`, `${exp}.${sig}x`, `${exp}.***`]) {
      expect(await verify(junk)).toBe(false);
    }
    expect(await verifyToken("another-secret", c.token, c.media_id, c.expires - 1)).toBe(false);
  });
});

/** Just enough of R2's `get` (with Range support) for the gateway. */
function fakeBucket(files: Record<string, Uint8Array>): R2Bucket {
  return {
    async get(key: string, opts?: R2GetOptions) {
      const data = files[key];
      if (!data) return null;
      const header = opts?.range instanceof Headers ? opts.range.get("range") : null;
      const m = header && /^bytes=(\d+)-(\d+)?$/.exec(header);
      const offset = m ? Number(m[1]) : 0;
      const length = m ? (m[2] ? Number(m[2]) : data.length - 1) - offset + 1 : data.length;
      const range: R2Range | undefined = m ? { offset, length } : undefined;
      const slice = data.slice(offset, offset + length);
      return { size: data.length, range, httpEtag: '"etag"', body: new Response(slice).body } as unknown as R2ObjectBody;
    },
  } as unknown as R2Bucket;
}

const MEDIA = "01a1a8ef-c77c-71d8-a97a-9bfcfa15d3aa";
const NOW = 1_800_000_000;
const segment = Uint8Array.from({ length: 256 }, (_, i) => i);
const env: Env = {
  MEDIA_TOKEN_SECRET: vectors.secret,
  MEDIA: fakeBucket({
    [`m/${MEDIA}/hls/master.m3u8`]: new TextEncoder().encode("#EXTM3U\n"),
    [`m/${MEDIA}/hls/v0/seg_0000.m4s`]: segment,
    [`m/${MEDIA}/original`]: new TextEncoder().encode("the original"),
  }),
};

async function get(path: string, init?: RequestInit, token?: string) {
  const t = token ?? (await signToken(vectors.secret, MEDIA, NOW + 3600));
  return handle(new Request(`https://media.example/${t}/m/${MEDIA}/${path}`, init), env, NOW);
}

describe("gateway", () => {
  it("serves playlists with the right headers", async () => {
    const res = await get("hls/master.m3u8");
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("application/vnd.apple.mpegurl");
    expect(res.headers.get("access-control-allow-origin")).toBe("*");
    expect(res.headers.get("x-content-type-options")).toBe("nosniff");
    expect(await res.text()).toBe("#EXTM3U\n");
  });

  it("serves byte ranges", async () => {
    const res = await get("hls/v0/seg_0000.m4s", { headers: { Range: "bytes=10-19" } });
    expect(res.status).toBe(206);
    expect(res.headers.get("content-range")).toBe("bytes 10-19/256");
    expect(new Uint8Array(await res.arrayBuffer())).toEqual(segment.slice(10, 20));
  });

  it("refuses bad tokens, originals, traversal and unknown files", async () => {
    expect((await get("hls/master.m3u8", undefined, `${NOW + 3600}.AAAAAAAAAAAAAAAAAAAAAA`)).status).toBe(403);
    expect((await get("hls/master.m3u8", undefined, await signToken(vectors.secret, MEDIA, NOW - 1))).status).toBe(403);
    for (const path of ["original", "hls/..%2Foriginal", "hls/../original", "hls/missing.m3u8", "hls/v0/evil.html"]) {
      expect((await get(path)).status, path).toBe(404);
    }
    const otherItem = await signToken(vectors.secret, "0196f5d2-0000-7000-8000-000000000001", NOW + 3600);
    expect((await get("hls/master.m3u8", undefined, otherItem)).status).toBe(403);
  });

  it("answers CORS preflights and rejects writes", async () => {
    const pre = await handle(new Request(`https://media.example/x/m/${MEDIA}/a.m3u8`, { method: "OPTIONS" }), env, NOW);
    expect(pre.status).toBe(204);
    expect(pre.headers.get("access-control-allow-headers")).toBe("Range");
    expect((await get("hls/master.m3u8", { method: "PUT", body: "x" })).status).toBe(405);
  });
});
