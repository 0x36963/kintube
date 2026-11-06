import createClient from "openapi-fetch";

import type { components, paths } from "./schema";

/**
 * Typed API client generated from the Rust OpenAPI document (`pnpm gen:api`).
 * Same-origin, so the session cookie is sent automatically.
 */
export const api = createClient<paths>({ baseUrl: "" });

export type Me = components["schemas"]["Me"];
type ErrorBody = components["schemas"]["ErrorBody"];

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

/** Unwraps an openapi-fetch result, throwing an `ApiError` for any error response. */
export function unwrap<T>(result: { data?: T; error?: unknown; response: Response }): T {
  if (result.response.ok && result.data !== undefined) return result.data;
  if (result.response.ok) return undefined as T; // e.g. 204 No Content
  const body = result.error as Partial<ErrorBody> | undefined;
  throw new ApiError(
    result.response.status,
    body?.error?.code ?? "unknown",
    body?.error?.message ?? `Request failed (${result.response.status})`,
  );
}

export function errorMessage(error: unknown): string {
  if (error instanceof ApiError) return error.message;
  return "Couldn't reach the server. Check your connection and try again.";
}
