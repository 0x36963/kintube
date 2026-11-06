import { useInfiniteQuery } from "@tanstack/react-query";
import { Upload } from "lucide-react";
import { Link } from "react-router";

import { UploadButton } from "~/components/uploads";
import { EmptyState, GhostGrid } from "~/components/ui";
import { VideoGrid } from "~/components/video-card";
import { APP_NAME } from "~/lib/brand";
import { libraryQuery } from "~/lib/media";
import { useUser } from "~/lib/session";

export function meta() {
  return [{ title: APP_NAME }];
}

export default function Home() {
  const user = useUser();
  const { data } = useInfiniteQuery(libraryQuery);
  const hasVideos = !!data?.pages[0]?.items.length;
  return (
    <div className="mx-auto max-w-[1800px]">
      <section className="pt-4 pb-6">
        <h1 className="text-2xl font-semibold tracking-tight">Welcome, {user.display_name}</h1>
        <p className="mt-1 text-sm text-fg-muted">Your private library of family videos and photos.</p>
      </section>
      {hasVideos && <h2 className="mb-4 text-lg font-semibold">Recent uploads</h2>}
      <VideoGrid
        limit={8}
        empty={
          <>
            <EmptyState
              icon={Upload}
              title="Your library is empty"
              action={
                <UploadButton className="inline-flex h-10 items-center gap-2 rounded-full bg-accent px-5 text-sm font-medium text-accent-fg hover:bg-accent-hover">
                  <Upload className="size-4" />
                  Upload videos
                </UploadButton>
              }
            >
              Upload your first videos, or drag them anywhere onto this page. Only you can see them.
            </EmptyState>
            <GhostGrid />
          </>
        }
      />
      {hasVideos && (
        <div className="mt-8 text-center">
          <Link to="/videos" className="text-sm font-medium text-accent hover:underline">
            See all videos
          </Link>
        </div>
      )}
    </div>
  );
}
