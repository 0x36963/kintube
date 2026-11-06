import { useLocation } from "react-router";

import { EmptyState, GhostGrid } from "~/components/ui";
import { APP_NAME } from "~/lib/brand";
import { comingSoonPages } from "~/lib/nav";

/** Shared placeholder for pages whose feature ships in a later milestone. */
export default function ComingSoon() {
  const { pathname } = useLocation();
  const page = comingSoonPages.find((p) => p.to === pathname);
  if (!page) throw new Response("Not found", { status: 404 });

  return (
    <div className="mx-auto max-w-[1800px]">
      <title>{`${page.label} · ${APP_NAME}`}</title>
      <EmptyState icon={page.icon} title={page.label} badge={`Coming in ${page.milestone}`}>
        {page.blurb}
      </EmptyState>
      <GhostGrid count={4} />
    </div>
  );
}
