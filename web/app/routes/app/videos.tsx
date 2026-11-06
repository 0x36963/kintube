import { Clapperboard, Upload } from "lucide-react";

import { UploadButton } from "~/components/uploads";
import { EmptyState } from "~/components/ui";
import { VideoGrid } from "~/components/video-card";
import { APP_NAME } from "~/lib/brand";

export function meta() {
  return [{ title: `Videos · ${APP_NAME}` }];
}

export default function Videos() {
  return (
    <div className="mx-auto max-w-[1800px] pt-4">
      <div className="mb-6 flex items-center justify-between gap-4">
        <h1 className="text-2xl font-semibold tracking-tight">Videos</h1>
      </div>
      <VideoGrid
        empty={
          <EmptyState
            icon={Clapperboard}
            title="No videos yet"
            action={
              <UploadButton className="inline-flex h-10 items-center gap-2 rounded-full bg-accent px-5 text-sm font-medium text-accent-fg hover:bg-accent-hover">
                <Upload className="size-4" />
                Upload videos
              </UploadButton>
            }
          >
            Upload videos from your computer, or drag them anywhere onto this page. They're private to you.
          </EmptyState>
        }
      />
    </div>
  );
}
