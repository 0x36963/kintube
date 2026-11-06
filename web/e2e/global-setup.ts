import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, renameSync } from "node:fs";
import path from "node:path";

import { CLIP, FIXTURES } from "./helpers";

/** Generates media fixtures once, before any test runs (tests run in parallel). */
export default function globalSetup() {
  mkdirSync(FIXTURES, { recursive: true });
  if (existsSync(CLIP)) return;
  // Write to a temp name and rename, so a half-written file is never visible.
  const tmp = path.join(FIXTURES, `.tmp-${process.pid}.mp4`);
  execFileSync("ffmpeg", [
    "-hide_banner", "-v", "error", "-y",
    "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30",
    "-f", "lavfi", "-i", "sine=frequency=440",
    "-t", "5", "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-c:a", "aac",
    tmp,
  ]);
  renameSync(tmp, CLIP);
}
