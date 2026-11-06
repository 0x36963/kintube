import { QueryCache, QueryClient, queryOptions } from "@tanstack/react-query";
import { createContext as createRouterContext } from "react-router";
import { createContext, useContext } from "react";

import { ApiError, api, unwrap, type Me } from "./api/client";

export const queryClient = new QueryClient({
  // A 401 from any query means the session ended (expired, or signed out elsewhere).
  queryCache: new QueryCache({
    onError: (error) => {
      if (error instanceof ApiError && error.status === 401) {
        queryClient.clear();
        window.location.assign(`/login?next=${encodeURIComponent(location.pathname + location.search)}`);
      }
    },
  }),
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      // Client errors (4xx) won't fix themselves on retry.
      retry: (count, error) => !(error instanceof ApiError && error.status < 500) && count < 2,
    },
  },
});

/** The signed-in user, or `null` when signed out. */
export const meQuery = queryOptions({
  queryKey: ["me"],
  queryFn: async (): Promise<Me | null> => {
    const result = await api.GET("/api/me");
    if (result.response.status === 401) return null;
    return unwrap(result);
  },
  staleTime: 5 * 60_000,
});

export function setSignedIn(me: Me) {
  queryClient.setQueryData(meQuery.queryKey, me);
}

export async function signOut() {
  unwrap(await api.POST("/api/auth/logout"));
  queryClient.clear();
}

/** Router context set by the app layout's auth middleware. */
export const userRouterContext = createRouterContext<Me>();

/** React context for components inside the signed-in app shell. */
export const UserContext = createContext<Me | null>(null);

export function useUser(): Me {
  const user = useContext(UserContext);
  if (!user) throw new Error("useUser() used outside the signed-in app layout");
  return user;
}

/** Only allow same-site relative paths as post-login redirects. */
export function safeNextPath(next: string | null): string {
  return next && next.startsWith("/") && !next.startsWith("//") ? next : "/";
}
