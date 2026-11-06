import { QueryClientProvider } from "@tanstack/react-query";
import { isRouteErrorResponse, Link, Links, Meta, Outlet, Scripts, ScrollRestoration } from "react-router";

import type { Route } from "./+types/root";
import "./app.css";
import { Logo, Spinner } from "./components/ui";
import { APP_NAME } from "./lib/brand";
import { queryClient } from "./lib/session";

export const links: Route.LinksFunction = () => [{ rel: "icon", href: "/favicon.svg", type: "image/svg+xml" }];

export function meta() {
  return [{ title: APP_NAME }];
}

export function Layout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />
        <meta name="color-scheme" content="dark" />
        <meta name="theme-color" content="#0e0e10" />
        <meta name="referrer" content="strict-origin-when-cross-origin" />
        <Meta />
        <Links />
      </head>
      <body>
        {children}
        <ScrollRestoration />
        <Scripts />
      </body>
    </html>
  );
}

export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <Outlet />
    </QueryClientProvider>
  );
}

/** Shown while the first client loaders (e.g. the session check) run. */
export function HydrateFallback() {
  return (
    <div className="grid h-dvh place-items-center">
      <div className="flex flex-col items-center gap-6">
        <Logo withName={false} className="scale-150" />
        <Spinner className="size-5" />
      </div>
    </div>
  );
}

export function ErrorBoundary({ error }: Route.ErrorBoundaryProps) {
  let title = "Something went wrong";
  let details = "An unexpected error occurred. Try reloading the page.";
  let stack: string | undefined;

  if (isRouteErrorResponse(error)) {
    title = error.status === 404 ? "Page not found" : `Error ${error.status}`;
    details = error.status === 404 ? "This page doesn't exist." : error.statusText || details;
  } else if (import.meta.env.DEV && error instanceof Error) {
    details = error.message;
    stack = error.stack;
  }

  return (
    <main className="mx-auto max-w-2xl px-6 py-24">
      <Logo />
      <h1 className="mt-10 text-2xl font-semibold">{title}</h1>
      <p className="mt-2 text-fg-muted">{details}</p>
      <Link to="/" className="mt-6 inline-block text-accent hover:underline">
        Go home
      </Link>
      {stack && (
        <pre className="mt-8 overflow-x-auto rounded-lg bg-surface p-4 text-xs text-fg-muted">
          <code>{stack}</code>
        </pre>
      )}
    </main>
  );
}
