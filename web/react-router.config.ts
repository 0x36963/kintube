import type { Config } from "@react-router/dev/config";

export default {
  // SPA mode: the app is static files on a CDN that talk to the Rust API.
  // Nothing private is server-rendered. See PLAN.md §2 "Why SPA mode".
  ssr: false,
} satisfies Config;
