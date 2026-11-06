import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Outlet, redirect } from "react-router";

import { MobileNav, Sidebar } from "~/components/sidebar";
import { TopBar } from "~/components/top-bar";
import { UserContext, meQuery, queryClient, userRouterContext } from "~/lib/session";

import type { Route } from "./+types/layout";

/** Auth guard for every page in the app shell. Runs before any child loader. */
export const clientMiddleware: Route.ClientMiddlewareFunction[] = [
  async ({ request, context }) => {
    const me = await queryClient.ensureQueryData(meQuery);
    if (!me) {
      const url = new URL(request.url);
      const next = url.pathname + url.search;
      throw redirect(next === "/" ? "/login" : `/login?next=${encodeURIComponent(next)}`);
    }
    context.set(userRouterContext, me);
  },
];

export async function clientLoader({ context }: Route.ClientLoaderArgs) {
  return { user: context.get(userRouterContext) };
}

export default function AppLayout({ loaderData }: Route.ComponentProps) {
  // Prefer the live cache entry so profile edits show up without a navigation.
  const user = useQuery(meQuery).data ?? loaderData.user;
  const [expanded, setExpanded] = useSidebarPreference();

  return (
    <UserContext.Provider value={user}>
      <TopBar onToggleSidebar={() => setExpanded(!expanded)} />
      <div className="flex">
        <Sidebar expanded={expanded} />
        <main className="min-w-0 flex-1 px-4 pt-2 pb-24 sm:px-6 md:pb-10">
          <Outlet />
        </main>
      </div>
      <MobileNav />
    </UserContext.Provider>
  );
}

const SIDEBAR_KEY = "sidebar";

/** Full guide on wide screens by default; the user's choice is remembered. */
function useSidebarPreference(): [boolean, (expanded: boolean) => void] {
  const [expanded, setExpanded] = useState(() => {
    const saved = localStorage.getItem(SIDEBAR_KEY);
    return saved ? saved === "expanded" : window.matchMedia("(min-width: 1280px)").matches;
  });
  useEffect(() => {
    localStorage.setItem(SIDEBAR_KEY, expanded ? "expanded" : "mini");
  }, [expanded]);
  return [expanded, setExpanded];
}
