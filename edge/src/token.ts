/**
 * Media path tokens: `{expires_unix}.{sig}`, where
 * `sig = base64url(HMAC-SHA256(secret, "v1:{media_id}:{expires_unix}")[0..16])`.
 * Must match backend/crates/app-core/src/media_token.rs; both are checked against
 * fixtures/media-token-vectors.json.
 */

const encoder = new TextEncoder();
const keys = new Map<string, Promise<CryptoKey>>();

function hmacKey(secret: string): Promise<CryptoKey> {
  let key = keys.get(secret);
  if (!key) {
    key = crypto.subtle.importKey("raw", encoder.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    keys.set(secret, key);
  }
  return key;
}

async function mac(secret: string, mediaId: string, expires: number): Promise<Uint8Array> {
  const full = await crypto.subtle.sign("HMAC", await hmacKey(secret), encoder.encode(`v1:${mediaId}:${expires}`));
  return new Uint8Array(full).slice(0, 16);
}

export async function signToken(secret: string, mediaId: string, expires: number): Promise<string> {
  return `${expires}.${base64url(await mac(secret, mediaId, expires))}`;
}

export async function verifyToken(secret: string, token: string, mediaId: string, nowSecs: number): Promise<boolean> {
  const dot = token.indexOf(".");
  const expiresText = token.slice(0, dot);
  if (dot < 1 || !/^\d{1,12}$/.test(expiresText)) return false;
  const expires = Number(expiresText);
  if (expires <= nowSecs) return false;
  const sig = fromBase64url(token.slice(dot + 1));
  if (!sig || sig.length !== 16) return false;
  return constantTimeEqual(sig, await mac(secret, mediaId.toLowerCase(), expires));
}

function constantTimeEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i]! ^ b[i]!;
  return diff === 0;
}

function base64url(bytes: Uint8Array): string {
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function fromBase64url(text: string): Uint8Array | null {
  if (!/^[A-Za-z0-9_-]*$/.test(text)) return null;
  try {
    const binary = atob(text.replaceAll("-", "+").replaceAll("_", "/"));
    return Uint8Array.from(binary, (c) => c.charCodeAt(0));
  } catch {
    return null;
  }
}
