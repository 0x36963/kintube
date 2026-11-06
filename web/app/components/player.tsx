import "@videojs/react/video/skin.css";

import { HlsJsVideo } from "@videojs/react/media/hlsjs-video";
import { VideoPlayer, VideoSkin } from "@videojs/react/video";

/**
 * Video.js v10 with the default skin (quality menu, scrub thumbnails, hotkeys, gestures),
 * themed to our tokens. HLS through hls.js, or the browser's own HLS on Safari/iOS.
 *
 * Media comes from a separate origin (the media gateway), hence `crossOrigin`; the
 * thumbnails track's sprite URLs are relative to the VTT, so they inherit its token.
 */
export function Player({
  hlsUrl,
  thumbnailsUrl,
  poster,
  autoPlay = true,
}: {
  hlsUrl: string;
  thumbnailsUrl?: string | null;
  poster?: string;
  autoPlay?: boolean;
}) {
  return (
    <VideoPlayer poster={poster}>
      <VideoSkin
        className="size-full"
        style={
          {
            "--media-accent-color": "var(--accent)",
            "--media-accent-text-color": "var(--accent-fg)",
            "--media-border-radius": "0.75rem",
            "--media-border-color": "transparent",
            "--media-font-family": "var(--font-sans)",
          } as React.CSSProperties
        }
      >
        <HlsJsVideo
          source={{ src: hlsUrl, type: "application/vnd.apple.mpegurl" }}
          crossOrigin="anonymous"
          playsInline
          autoPlay={autoPlay}
          data-testid="video"
        >
          {thumbnailsUrl && <track kind="metadata" label="thumbnails" src={thumbnailsUrl} default />}
        </HlsJsVideo>
      </VideoSkin>
    </VideoPlayer>
  );
}
