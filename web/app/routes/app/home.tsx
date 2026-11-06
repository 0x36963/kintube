import { Upload } from "lucide-react";

import { EmptyState, GhostGrid } from "~/components/ui";
import { APP_NAME } from "~/lib/brand";
import { useUser } from "~/lib/session";

export function meta() {
  return [{ title: APP_NAME }];
}

export default function Home() {
  const user = useUser();
  return (
    <div className="mx-auto max-w-[1800px]">
      <section className="pt-4 pb-2">
        <h1 className="text-2xl font-semibold tracking-tight">Welcome, {user.display_name}</h1>
        <p className="mt-1 text-sm text-fg-muted">Your private library of family videos and photos.</p>
      </section>
      <EmptyState icon={Upload} title="Your library is empty" badge="Uploads arrive in M1">
        Once uploads are switched on, your videos and photos will show up here, with continue watching,
        recent uploads, and a shelf for each channel.
      </EmptyState>
      <GhostGrid />
    </div>
  );
}
