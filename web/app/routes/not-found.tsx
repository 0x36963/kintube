import { Link } from "react-router";

import { Logo } from "~/components/ui";
import { APP_NAME } from "~/lib/brand";

export function meta() {
  return [{ title: `Not found · ${APP_NAME}` }];
}

export default function NotFound() {
  return (
    <main className="grid min-h-dvh place-items-center px-4">
      <div className="text-center">
        <Logo className="mb-10 scale-125" />
        <h1 className="text-2xl font-semibold">Page not found</h1>
        <p className="mt-2 text-fg-muted">This page doesn't exist, or it may have moved.</p>
        <Link to="/" className="mt-6 inline-block text-accent hover:underline">
          Go home
        </Link>
      </div>
    </main>
  );
}
